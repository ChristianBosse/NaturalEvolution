//! Routes and handlers.

use std::collections::HashSet;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::extract::multipart::Field;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Request, State};
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use nevo_format::{seal, ContentFile, Id, Keypair, Kind, Revision, Sealed, Time, TimeSource, Topic, Visibility};
use nevo_store::{BlobInfo, Store, StoreError};
use tokio::sync::{mpsc, Semaphore};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::accounts::{Accounts, Session};
use crate::auth::{self, Contributor, Viewer};
use crate::{account_pages, edit, markdown, pages, setup};

/// Largest upload accepted in one form: enough for a long video. Computed in u64 so it also
/// compiles on 32-bit boards (armv7 Raspberry Pi OS), where it is capped at the address space.
const MAX_UPLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_UPLOAD: usize = if MAX_UPLOAD_BYTES > usize::MAX as u64 {
    usize::MAX
} else {
    MAX_UPLOAD_BYTES as usize
};
/// Largest guide text, and the largest Markdown file rendered inline.
pub(crate) const MAX_TEXT: usize = 1024 * 1024;
pub(crate) const MAX_FIELD: usize = 4096;
const MAX_TITLE_CHARS: usize = 500;
/// Limits on one form, so a single request can't exhaust a Pi's memory or CPU.
pub(crate) const MAX_PARTS: usize = 200;
pub(crate) const MAX_FILES: usize = 100;
const MAX_TOPICS: usize = 16;
/// Uploads written at the same time. Each holds a blocking thread while it streams.
const MAX_CONCURRENT_UPLOADS: usize = 4;
/// An upload that sends nothing for this long is dropped (a phone that left the Wi-Fi).
const UPLOAD_IDLE: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    /// This node's key. It signs the node's log; people sign what they add with their own keys.
    pub node: Arc<Keypair>,
    pub accounts: Arc<Accounts>,
    /// Held while choosing a clock value and storing the object, so two additions at the same
    /// moment can't get the same clock (spec section 2: each new object needs a higher one).
    pub(crate) adding: Arc<Mutex<()>>,
    uploads: Arc<Semaphore>,
}

impl AppState {
    pub fn new(store: Arc<Store>, node: Arc<Keypair>, accounts: Arc<Accounts>) -> AppState {
        AppState {
            store,
            node,
            accounts,
            adding: Arc::new(Mutex::new(())),
            uploads: Arc::new(Semaphore::new(MAX_CONCURRENT_UPLOADS)),
        }
    }
}

pub fn router(state: AppState) -> Router {
    let csp = "default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; media-src 'self'; \
               form-action 'self'; frame-ancestors 'none'; base-uri 'none'";
    Router::new()
        .route("/", get(home))
        .route("/topics", get(topics))
        .route("/topic/{*path}", get(topic))
        .route("/item/{id}", get(item))
        .route("/file/{rev}/{index}/{name}", get(file))
        .route("/add", get(add_form).post(add_submit))
        .route("/item/{id}/edit", get(edit::edit_form).post(edit::edit_submit))
        .route("/item/{id}/merge", get(edit::merge_form))
        .route("/item/{id}/history", get(edit::history))
        .route("/item/{id}/version/{rev}", get(edit::version))
        .route("/health", get(|| async { "ok" }))
        .merge(auth::routes())
        .merge(setup::routes())
        .merge(account_pages::routes())
        .layer(middleware::from_fn_with_state(state.clone(), auth::middleware))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(csp),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            // Not `no-referrer`: with it, browsers send `Origin: null` on form posts, and the
            // forged-form check (auth.rs) needs the real origin.
            HeaderValue::from_static("same-origin"),
        ))
        .with_state(state)
}

// ---------------------------------------------------------------------------------------------
// Errors

pub enum AppError {
    NotFound,
    BadRequest(String),
    /// Free space is below the store's reserve, so new files are refused.
    DiskFull,
    /// Not allowed: a reader adding, a forged form, a non-admin managing accounts.
    Forbidden(String),
    Internal(String),
}

const DISK_FULL: &str = "This NEVO is almost out of storage space, so it can't take new files right now. \
                         Please tell the person who looks after it.";

impl From<StoreError> for AppError {
    fn from(e: StoreError) -> AppError {
        match e {
            StoreError::DiskFull { .. } => AppError::DiskFull,
            // Objects in the store were checked when stored, so even a format error here means
            // something is wrong with this node, not with the request.
            e => AppError::Internal(e.to_string()),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                "Nothing here. It may not have reached this NEVO yet.".into(),
            ),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::DiskFull => (StatusCode::INSUFFICIENT_STORAGE, DISK_FULL.into()),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            AppError::Internal(m) => {
                eprintln!("error: {m}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong on this NEVO. Details are in its log.".into(),
                )
            }
        };
        (status, Html(pages::error(status.as_u16(), &message).into_string())).into_response()
    }
}

pub(crate) type AppResult<T> = Result<T, AppError>;

/// Runs store work on the blocking thread pool: SQLite and file I/O must not stall the async
/// workers that serve other people.
pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
}

pub(crate) fn parse_id(text: &str) -> AppResult<Id> {
    text.parse().map_err(|_| AppError::NotFound)
}

// ---------------------------------------------------------------------------------------------
// Pages

/// The newest version of an item that the viewer may see. Private items are shown only to
/// their author.
pub(crate) fn current<'a>(tips: &'a [Sealed<Revision>], viewer: &Viewer) -> Option<&'a Sealed<Revision>> {
    tips.iter()
        .filter(|r| viewer.can_see(&r.object))
        .max_by_key(|r| r.object.clock)
}

async fn home(State(state): State<AppState>, viewer: Viewer) -> AppResult<Html<String>> {
    let (recent, free) = blocking(move || {
        let tips = state.store.recent_tips(100)?;
        let recent = tips
            .into_iter()
            .filter(|r| viewer.can_see(&r.object))
            .take(50)
            .map(|r| (r.object.item_id(r.id()), r))
            .collect::<Vec<_>>();
        Ok((recent, state.store.free_space().ok()))
    })
    .await?;
    Ok(Html(pages::home(&recent, free).into_string()))
}

async fn topics() -> Html<String> {
    Html(pages::topics().into_string())
}

async fn topic(State(state): State<AppState>, viewer: Viewer, Path(path): Path<String>) -> AppResult<Html<String>> {
    let topic: Topic = path.parse().map_err(|_| AppError::NotFound)?;
    let shown = topic.clone();
    let items = blocking(move || {
        let mut out = Vec::new();
        for item in state.store.items_in_topic(&topic)? {
            if let Some(tip) = current(&state.store.item_tips(item)?, &viewer) {
                out.push((item, tip.clone()));
            }
        }
        out.sort_by_key(|(_, r)| r.object.title.to_lowercase());
        Ok(out)
    })
    .await?;
    Ok(Html(pages::topic(&shown, &items).into_string()))
}

async fn item(State(state): State<AppState>, viewer: Viewer, Path(id): Path<String>) -> AppResult<Html<String>> {
    let item = parse_id(&id)?;
    let page = blocking(move || {
        let store = &state.store;
        let tips = store.item_tips(item)?;
        let tip = current(&tips, &viewer).ok_or(AppError::NotFound)?.clone();
        let visible_tips = tips.iter().filter(|r| viewer.can_see(&r.object)).count();
        let history: Vec<_> = store
            .item_revisions(item)?
            .into_iter()
            .filter(|r| viewer.can_see(&r.object))
            .collect();

        // The first Markdown file is the guide text, shown on the page.
        let rev_id = tip.id();
        let content = &tip.object.content;
        let guide_html = match content
            .iter()
            .find(|f| f.mime == "text/markdown" && f.size <= MAX_TEXT as u64)
        {
            Some(f) => match store.open_blob(f.blob)? {
                Some(mut file) => {
                    let mut bytes = Vec::new();
                    file.by_ref()
                        .take(MAX_TEXT as u64)
                        .read_to_end(&mut bytes)
                        .map_err(|e| AppError::Internal(e.to_string()))?;
                    // Old text files are often not UTF-8; show them anyway rather than fail.
                    let text = String::from_utf8_lossy(&bytes);
                    Some(markdown::render(&text, |name| {
                        content
                            .iter()
                            .position(|c| c.name == name)
                            .map(|i| pages::file_url(rev_id, i, &content[i]))
                    }))
                }
                None => None,
            },
            None => None,
        };
        let view = pages::ItemView {
            item,
            tip: &tip,
            tips: visible_tips,
            history: &history,
            local_author: tip.object.author == state.node.public()
                || state.accounts.is_local_key(&tip.object.author)?,
            guide_html,
        };
        Ok(pages::item(&view).into_string())
    })
    .await?;
    Ok(Html(page))
}

/// Media types a browser may display inline. Anything else (HTML, SVG, scripts, unknown
/// types) is downloaded, so an uploaded file can't run code in the NEVO page.
fn inline_type(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "image/png",
        "image/jpeg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "application/pdf" => "application/pdf",
        "text/plain" | "text/markdown" => "text/plain; charset=utf-8",
        m if m.starts_with("video/") || m.starts_with("audio/") => return media_type(m),
        _ => return None,
    })
}

fn media_type(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "video/mp4" => "video/mp4",
        "video/webm" => "video/webm",
        "video/ogg" => "video/ogg",
        "audio/mpeg" => "audio/mpeg",
        "audio/ogg" => "audio/ogg",
        "audio/webm" => "audio/webm",
        "audio/wav" | "audio/x-wav" => "audio/wav",
        "audio/mp4" => "audio/mp4",
        _ => return None,
    })
}

/// Serves one file of a revision. Range requests work, so video and audio can seek.
async fn file(
    State(state): State<AppState>,
    viewer: Viewer,
    Path((rev, index, _name)): Path<(String, usize, String)>,
    request: Request,
) -> AppResult<Response> {
    let rev = parse_id(&rev)?;
    let (path, file, private) = blocking(move || {
        let revision = match state.store.get::<Revision>(rev) {
            Ok(Some(r)) => r,
            // Not stored, or the ID of something that isn't a revision.
            Ok(None) | Err(StoreError::Format(_)) => return Err(AppError::NotFound),
            Err(e) => return Err(e.into()),
        };
        if !viewer.can_see(&revision.object) {
            return Err(AppError::NotFound);
        }
        let file = revision.object.content.get(index).cloned().ok_or(AppError::NotFound)?;
        let path = state.store.blob_path(file.blob);
        if !path.is_file() {
            return Err(AppError::NotFound);
        }
        Ok((path, file, revision.object.vis == Visibility::Private))
    })
    .await?;

    let mut response = ServeFile::new(path)
        .oneshot(request)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map(Body::new);
    let headers = response.headers_mut();
    let ascii_name: String = file
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || ".-_ ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    match inline_type(&file.mime) {
        Some(mime) => {
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
            headers.insert(header::CONTENT_DISPOSITION, disposition("inline", &ascii_name));
        }
        None => {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            headers.insert(header::CONTENT_DISPOSITION, disposition("attachment", &ascii_name));
        }
    }
    // Files never change (their name is their hash), so browsers may keep them. Not private
    // ones: the next person on a shared device must not find them in the cache.
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if private {
            "no-store"
        } else {
            "private, max-age=31536000, immutable"
        }),
    );
    // Only types that can't run code are shown inline, so a strict policy is enough. (A CSP
    // `sandbox` would be stricter but stops browsers' built-in PDF viewers.)
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static("default-src 'none'; img-src 'self'; media-src 'self'; style-src 'unsafe-inline'"),
    );
    Ok(response)
}

fn disposition(kind: &str, ascii_name: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("{kind}; filename=\"{ascii_name}\""))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

// ---------------------------------------------------------------------------------------------
// Adding a guide

async fn add_form(Contributor(session): Contributor) -> Html<String> {
    let form = pages::AddForm {
        csrf: session.csrf_token(),
        ..Default::default()
    };
    Html(pages::add(&form).into_string())
}

async fn add_submit(
    State(state): State<AppState>,
    Contributor(session): Contributor,
    multipart: Multipart,
) -> Response {
    let mut form = pages::AddForm::default();
    let result = add(&state, &session, multipart, &mut form).await;
    form.csrf = session.csrf_token();
    match result {
        Ok(item) => Redirect::to(&format!("/item/{item}")).into_response(),
        Err(AppError::BadRequest(message)) => {
            form.error = Some(message);
            (StatusCode::BAD_REQUEST, Html(pages::add(&form).into_string())).into_response()
        }
        Err(AppError::DiskFull) => {
            form.error = Some(DISK_FULL.into());
            (StatusCode::INSUFFICIENT_STORAGE, Html(pages::add(&form).into_string())).into_response()
        }
        Err(other) => other.into_response(),
    }
}

async fn add(
    state: &AppState,
    session: &Session,
    mut multipart: Multipart,
    form: &mut pages::AddForm,
) -> AppResult<Id> {
    let bad = |m: &str| AppError::BadRequest(m.to_string());
    let mut files = Vec::new();
    let mut parts = 0;
    let mut checked = false;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        parts += 1;
        if parts > MAX_PARTS {
            return Err(bad("The form has too many parts."));
        }
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "files" => {
                let Some(file_name) = upload_name(&field) else {
                    continue;
                };
                // The form puts files last, so the typed fields have arrived: check them before
                // storing anything, so a mistake doesn't leave uploaded files behind.
                if !checked {
                    auth::check_session_csrf(session, &form.csrf)?;
                    check_fields(form)?;
                    checked = true;
                }
                if files.len() >= MAX_FILES {
                    return Err(AppError::BadRequest(format!("At most {MAX_FILES} files at a time.")));
                }
                if let Some(file) = upload(state, field, file_name).await? {
                    files.push(file);
                }
            }
            _ => typed_field(&name, field, form).await?,
        }
    }

    auth::check_session_csrf(session, &form.csrf)?;
    let (topics, text) = check_content(form, !files.is_empty())?;
    // Two attachments with the same name would make links to them ambiguous.
    let mut names = HashSet::new();
    for f in &mut files {
        f.name = unique_name(&f.name, &mut names);
    }

    let store = state.store.clone();
    let node = state.node.clone();
    // Signed by the person adding it; the node key only signs the node's log.
    let author_key = session.signing_key()?;
    let adding = state.adding.clone();
    let (lang, license, summary, title, node_only, private) = (
        form.lang.clone(),
        form.license.clone(),
        form.summary.clone(),
        form.title.clone(),
        form.node_only,
        form.private,
    );
    blocking(move || {
        let mut content = Vec::new();
        if !text.trim().is_empty() {
            let info = store.put_blob(text.as_bytes())?;
            content.push(ContentFile {
                name: unique_name("guide.md", &mut names),
                blob: info.id,
                size: info.size,
                mime: "text/markdown".into(),
            });
        }
        let kind = if content.is_empty() && files.iter().all(|f| f.mime.starts_with("video/")) {
            Kind::Video
        } else if content.is_empty() {
            Kind::Document
        } else {
            Kind::Guide
        };
        content.extend(files);
        let author = author_key.public();
        let _adding = adding.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let revision = Revision {
            item: None,
            parents: vec![],
            author,
            clock: store.author_clock(author)? + 1,
            time: now(),
            kind,
            title,
            lang: if lang.is_empty() { "en".into() } else { lang },
            topics,
            license: if license.is_empty() {
                "CC-BY-SA-4.0".into()
            } else {
                license
            },
            vis: if private {
                Visibility::Private
            } else if node_only {
                Visibility::Node
            } else {
                Visibility::Public
            },
            content,
            source: None,
            summary: (!summary.is_empty()).then_some(summary),
            extra: Default::default(),
        };
        let sealed =
            seal(&revision, &author_key).map_err(|e| AppError::BadRequest(format!("Please check the form: {e}")))?;
        if node_only || private {
            store.put_object(sealed.bytes())?;
        } else {
            store.publish(&node, sealed.bytes())?;
        }
        Ok(sealed.id())
    })
    .await
}

/// Reads one of the typed fields that the add and edit forms share into `form`. Other fields
/// are ignored.
pub(crate) async fn typed_field(name: &str, field: Field<'_>, form: &mut pages::AddForm) -> AppResult<()> {
    match name {
        "title" => form.title = text_field(field, MAX_FIELD).await?.trim().to_string(),
        "summary" => form.summary = text_field(field, MAX_FIELD).await?.trim().to_string(),
        "text" => form.text = text_field(field, MAX_TEXT).await?,
        "topics" => {
            if form.topics.len() >= MAX_TOPICS {
                return Err(AppError::BadRequest(format!("At most {MAX_TOPICS} topics.")));
            }
            form.topics.push(text_field(field, MAX_FIELD).await?);
        }
        "lang" => form.lang = text_field(field, MAX_FIELD).await?.trim().to_string(),
        "license" => form.license = text_field(field, MAX_FIELD).await?.trim().to_string(),
        "node_only" => form.node_only = true,
        "private" => form.private = true,
        "csrf" => form.csrf = text_field(field, MAX_FIELD).await?,
        _ => {}
    }
    Ok(())
}

/// The cleaned name of an uploaded file, or `None` for an empty file input (the browser sends
/// an empty part when no file was chosen).
pub(crate) fn upload_name(field: &Field<'_>) -> Option<String> {
    field.file_name().map(safe_file_name).filter(|name| !name.is_empty())
}

/// Streams one uploaded file into the blob store. An empty file is stored nowhere and gives
/// `None`.
pub(crate) async fn upload(state: &AppState, field: Field<'_>, name: String) -> AppResult<Option<ContentFile>> {
    let mime = field
        .content_type()
        .filter(|m| valid_mime(m))
        .unwrap_or("application/octet-stream")
        .to_string();
    let _permit = state
        .uploads
        .acquire()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let info = store_upload(state.store.clone(), field).await?;
    Ok((info.size > 0).then_some(ContentFile {
        name,
        blob: info.id,
        size: info.size,
        mime,
    }))
}

/// The checks that run once the whole form has arrived. Gives the topics, and the guide text
/// with the browser's line endings made plain.
pub(crate) fn check_content(form: &pages::AddForm, has_files: bool) -> AppResult<(Vec<Topic>, String)> {
    check_fields(form)?;
    let topics = form
        .topics
        .iter()
        .map(|t| {
            t.parse::<Topic>()
                .map_err(|_| AppError::BadRequest("Unknown topic.".into()))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let text = form.text.replace("\r\n", "\n");
    if text.trim().is_empty() && !has_files {
        return Err(AppError::BadRequest(
            "Write some text or attach at least one file.".into(),
        ));
    }
    Ok((topics, text))
}

pub(crate) fn now() -> Time {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    // A node's clock may be wrong offline (a Pi has no clock battery), so the source is unknown.
    Time {
        ms,
        src: TimeSource::Unknown,
    }
}

pub(crate) async fn text_field(mut field: Field<'_>, max: usize) -> AppResult<String> {
    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        if bytes.len() + chunk.len() > max {
            return Err(AppError::BadRequest(format!(
                "A field is longer than {} KB.",
                max / 1024
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| AppError::BadRequest("Text must be UTF-8.".into()))
}

/// Checks the typed fields. Runs before files are stored and again at the end.
pub(crate) fn check_fields(form: &pages::AddForm) -> AppResult<()> {
    if form.title.is_empty() {
        return Err(AppError::BadRequest("Please give it a title.".into()));
    }
    if form.title.chars().count() > MAX_TITLE_CHARS {
        return Err(AppError::BadRequest(format!(
            "The title can have at most {MAX_TITLE_CHARS} characters."
        )));
    }
    if form.topics.is_empty() {
        return Err(AppError::BadRequest("Please pick at least one topic.".into()));
    }
    Ok(())
}

/// `name` if unused, else `name (2)`, `name (3)`, … before the extension.
pub(crate) fn unique_name(name: &str, used: &mut HashSet<String>) -> String {
    if used.insert(name.to_string()) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| used.insert(candidate.clone()))
        .expect("an unused name exists")
}

/// Keeps only the file name part and removes characters that are unsafe in file names.
fn safe_file_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && *c != ':')
        .take(200)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        String::new()
    } else if cleaned == "." || cleaned == ".." {
        "file".into()
    } else {
        cleaned
    }
}

fn valid_mime(m: &str) -> bool {
    m.contains('/') && m.len() <= 100 && m.bytes().all(|b| b.is_ascii_graphic())
}

/// Streams an uploaded file into the blob store without holding it in memory.
async fn store_upload(store: Arc<Store>, mut field: Field<'_>) -> AppResult<BlobInfo> {
    let (tx, rx) = mpsc::channel::<Chunk>(8);
    let writer = tokio::task::spawn_blocking(move || {
        store.put_blob(ChannelReader {
            rx,
            current: Bytes::new(),
        })
    });
    // If this future is dropped (the browser went away), the channel closes without `Done`
    // and the writer discards the partial file.
    let mut failed = None;
    loop {
        match tokio::time::timeout(UPLOAD_IDLE, field.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                if tx.send(Chunk::Data(chunk)).await.is_err() {
                    break; // the writer stopped; its error is reported below
                }
            }
            Ok(Ok(None)) => {
                let _ = tx.send(Chunk::Done).await;
                break;
            }
            Ok(Err(e)) => {
                failed = Some(e.to_string());
                break;
            }
            Err(_) => {
                failed = Some("no data arrived for two minutes".into());
                break;
            }
        }
    }
    drop(tx);
    let stored = writer.await.map_err(|e| AppError::Internal(e.to_string()))?;
    if let Some(e) = failed {
        return Err(AppError::BadRequest(format!("The upload was interrupted: {e}")));
    }
    Ok(stored?)
}

enum Chunk {
    Data(Bytes),
    /// The whole file arrived. Without it, a closed channel means the upload broke off.
    Done,
}

/// Lets the blocking blob writer read chunks that the async upload handler receives.
struct ChannelReader {
    rx: mpsc::Receiver<Chunk>,
    current: Bytes,
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.current.is_empty() {
            match self.rx.blocking_recv() {
                Some(Chunk::Data(chunk)) => self.current = chunk,
                Some(Chunk::Done) => return Ok(0),
                None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "upload interrupted")),
            }
        }
        let n = buf.len().min(self.current.len());
        buf[..n].copy_from_slice(&self.current[..n]);
        self.current = self.current.slice(n..);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(safe_file_name("C:\\Users\\me\\photo.jpg"), "photo.jpg");
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name(".."), "file");
        assert_eq!(safe_file_name(""), "");
        assert_eq!(safe_file_name("a:b\u{0}.txt"), "ab.txt");
    }

    #[test]
    fn duplicate_names_get_numbers() {
        let mut used = HashSet::new();
        assert_eq!(unique_name("photo.jpg", &mut used), "photo.jpg");
        assert_eq!(unique_name("photo.jpg", &mut used), "photo (2).jpg");
        assert_eq!(unique_name("photo.jpg", &mut used), "photo (3).jpg");
        assert_eq!(unique_name("README", &mut used), "README");
        assert_eq!(unique_name("README", &mut used), "README (2)");
        assert_eq!(unique_name(".hidden", &mut used), ".hidden");
        assert_eq!(unique_name(".hidden", &mut used), ".hidden (2)");
    }

    // The upload limit fits the platform (checked when compiling, also for 32-bit targets).
    const _: () = assert!(MAX_UPLOAD as u64 <= MAX_UPLOAD_BYTES && MAX_UPLOAD >= 1 << 30);

    #[test]
    fn only_safe_types_display_inline() {
        assert_eq!(inline_type("image/png"), Some("image/png"));
        assert_eq!(inline_type("text/markdown"), Some("text/plain; charset=utf-8"));
        assert_eq!(inline_type("video/mp4"), Some("video/mp4"));
        for unsafe_type in [
            "text/html",
            "image/svg+xml",
            "application/javascript",
            "video/x-unknown",
            "application/octet-stream",
        ] {
            assert_eq!(inline_type(unsafe_type), None, "{unsafe_type}");
        }
    }
}
