//! Editing items, merging offline branches, and item history.
//!
//! Edits never overwrite (spec section 2). Saving makes a new revision whose parents are the
//! versions it builds on. Two people who edit the same version offline make two branch tips;
//! a merge is a revision with every tip as a parent.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use maud::{html, Markup, PreEscaped};
use nevo_format::{seal, ContentFile, Id, Keypair, Kind, Revision, Sealed, Visibility};
use nevo_store::{Store, StoreError};

use crate::accounts::Session;
use crate::app::{self, blocking, parse_id, AppError, AppResult, AppState, MAX_FIELD, MAX_FILES, MAX_PARTS, MAX_TEXT};
use crate::auth::{self, Contributor, Viewer};
use crate::diff::{self, Side};
use crate::{markdown, pages};

/// Most versions one merge joins. An item with more tips is merged in several steps.
const MAX_PARENTS: usize = 16;
/// Versions per history page. Each shows a diff, which needs its text and its parents' texts.
const HISTORY_PAGE: usize = 10;

const CSS: &str = r#"
.diff { font: .88rem/1.45 ui-monospace, Menlo, Consolas, monospace; border: 1px solid var(--line);
        border-radius: .3rem; margin: .5rem 0; overflow-x: auto; }
.diff div { white-space: pre-wrap; overflow-wrap: anywhere; padding: 0 .5rem; }
.diff .add, td.add { background: rgba(46, 160, 67, .2); }
.diff .del, td.del { background: rgba(248, 81, 73, .2); }
.diff .gap, .diff .sign { color: var(--muted); }
.scroll { overflow-x: auto; }
table.compare { width: 100%; font-size: .92rem; margin: .5rem 0; }
table.compare th { text-align: left; vertical-align: top; font-weight: 600; }
table.compare td { vertical-align: top; overflow-wrap: anywhere; }
tr.differs td { background: var(--warn-bg); color: var(--warn-fg); }
fieldset { border: 0; padding: 0; margin: 1rem 0 0; }
legend { font-weight: 600; padding: 0; }
form label.check { font-weight: 400; margin-top: .3rem; }
details { margin: .4rem 0; } summary { cursor: pointer; color: var(--accent); }
"#;

/// The key that signs edits and merges: the signed-in person's own (accounts.rs). The node
/// key only signs the node's log.
fn signer(session: &Session) -> AppResult<Arc<Keypair>> {
    Ok(Arc::new(session.signing_key()?))
}

// ---------------------------------------------------------------------------------------------
// Loading versions

/// A revision with its guide text read, as the edit, merge and history pages need it.
struct Version {
    rev: Sealed<Revision>,
    /// Index in `content` of the file edited as the guide text. Only a Markdown file that is
    /// stored here and is UTF-8 qualifies; any other file is offered as a file to keep, so an
    /// edit never mangles bytes it can't show.
    guide: Option<usize>,
    text: String,
}

impl Version {
    fn id(&self) -> Id {
        self.rev.id()
    }

    fn side(&self) -> Side<'_> {
        Side {
            rev: &self.rev.object,
            guide: self.guide,
            text: &self.text,
        }
    }

    /// The files other than the guide text, with their index in `content`.
    fn other_files(&self) -> impl Iterator<Item = (usize, &ContentFile)> {
        let guide = self.guide;
        self.rev
            .object
            .content
            .iter()
            .enumerate()
            .filter(move |(i, _)| Some(*i) != guide)
    }

    fn label(&self) -> String {
        let r = &self.rev.object;
        format!(
            "Version {} · {} · by {}",
            r.clock,
            pages::date(r.time.ms),
            pages::short_key(&r.author)
        )
    }
}

/// The guide file of a revision, by the item page's rule: the first Markdown file that is
/// small enough to show.
fn guide_file(rev: &Revision) -> Option<(usize, &ContentFile)> {
    rev.content
        .iter()
        .enumerate()
        .find(|(_, f)| f.mime == "text/markdown" && f.size <= MAX_TEXT as u64)
}

/// Loads a version's guide text. `texts` remembers texts already read, by blob.
fn load(store: &Store, rev: Sealed<Revision>, texts: &mut HashMap<Id, Option<String>>) -> AppResult<Version> {
    let Some((index, blob)) = guide_file(&rev.object).map(|(i, f)| (i, f.blob)) else {
        return Ok(Version {
            rev,
            guide: None,
            text: String::new(),
        });
    };
    let text = match texts.get(&blob) {
        Some(text) => text.clone(),
        None => {
            let text = read_text(store, blob)?;
            texts.insert(blob, text.clone());
            text
        }
    };
    Ok(match text {
        Some(text) => Version {
            rev,
            guide: Some(index),
            text,
        },
        None => Version {
            rev,
            guide: None,
            text: String::new(),
        },
    })
}

/// A blob as text, or `None` if it isn't stored here or isn't UTF-8.
fn read_text(store: &Store, blob: Id) -> AppResult<Option<String>> {
    let Some(file) = store.open_blob(blob)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(MAX_TEXT as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(String::from_utf8(bytes).ok())
}

/// The item's visible branch tips, the one the item page shows first.
fn visible_tips(store: &Store, item: Id) -> AppResult<Vec<Sealed<Revision>>> {
    let mut tips: Vec<_> = store
        .item_tips(item)?
        .into_iter()
        .filter(|r| r.object.vis != Visibility::Private)
        .collect();
    let shown = app::current(&tips, &Viewer::default()).ok_or(AppError::NotFound)?.id();
    if let Some(pos) = tips.iter().position(|t| t.id() == shown) {
        let tip = tips.remove(pos);
        tips.insert(0, tip);
    }
    Ok(tips)
}

/// Loads the versions a form builds on and checks that each belongs to the item.
fn parent_revisions(store: &Store, item: Id, ids: &[Id]) -> AppResult<Vec<Sealed<Revision>>> {
    let stale = || AppError::BadRequest("This form doesn't match the item any more. Please open it again.".into());
    if ids.is_empty() {
        return Err(stale());
    }
    ids.iter()
        .map(|&id| match store.get::<Revision>(id) {
            Ok(Some(r)) if r.object.item_id(r.id()) == item && r.object.vis != Visibility::Private => Ok(r),
            Ok(_) | Err(StoreError::Format(_)) => Err(stale()),
            Err(e) => Err(e.into()),
        })
        .collect()
}

/// Whether a new version must stay on this node. A version that builds on node-only content
/// can't be published (the store refuses it), and a node-only item must not leak out through
/// an edit.
fn stays_on_node(store: &Store, item: Id, parents: &[Sealed<Revision>]) -> AppResult<bool> {
    if parents.iter().any(|p| p.object.vis != Visibility::Public) {
        return Ok(true);
    }
    match store.get::<Revision>(item) {
        Ok(Some(first)) => Ok(first.object.vis != Visibility::Public),
        // The first version hasn't reached this node; its children here are public.
        Ok(None) | Err(StoreError::Format(_)) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------------------------------------
// The edit and merge form

/// What the edit form holds, also shown again after a mistake.
#[derive(Default)]
struct EditForm {
    /// The fields shared with the add form.
    fields: pages::AddForm,
    /// The versions this one builds on: one for an edit, every tip for a merge.
    parents: Vec<Id>,
    /// Files carried over from the parents, as `revision:index`.
    keep: Vec<String>,
}

fn keep_key(rev: Id, index: usize) -> String {
    format!("{rev}:{index}")
}

/// Files that can be kept: the parents' files other than their guide texts, each distinct
/// file once.
fn keep_choices(versions: &[Version]) -> Vec<(String, &ContentFile)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for v in versions {
        for (i, f) in v.other_files() {
            if seen.insert((&f.name, f.blob)) {
                out.push((keep_key(v.id(), i), f));
            }
        }
    }
    out
}

/// A form filled in from `versions[0]`, building on every one of `versions`. Every file is
/// kept, so a file added on one branch isn't lost by default in a merge.
fn prefill(versions: &[Version]) -> EditForm {
    let start = &versions[0];
    let r = &start.rev.object;
    EditForm {
        fields: pages::AddForm {
            title: r.title.clone(),
            summary: r.summary.clone().unwrap_or_default(),
            text: start.text.clone(),
            topics: r.topics.iter().map(|t| t.as_str().to_string()).collect(),
            lang: r.lang.clone(),
            license: r.license.clone(),
            node_only: r.vis != Visibility::Public,
            // Private items can't be edited yet (visible_tips leaves them out).
            private: false,
            // Filled in by the handler from the session.
            csrf: String::new(),
            error: None,
        },
        parents: versions.iter().map(Version::id).collect(),
        keep: keep_choices(versions).into_iter().map(|(key, _)| key).collect(),
    }
}

fn version_url(item: Id, rev: Id) -> String {
    format!("/item/{item}/version/{rev}")
}

/// The edit form; with several versions, the merge page.
fn edit_page(item: Id, versions: &[Version], form: &EditForm, on_node: bool) -> Markup {
    let merging = versions.len() > 1;
    let heading = if merging { "Compare and merge" } else { "Edit" };
    let choices = keep_choices(versions);
    pages::layout(
        heading,
        html! {
            style { (PreEscaped(CSS)) }
            p.muted { a href={ "/item/" (item) } { (versions[0].rev.object.title) } " / " (heading) }
            h1 { (heading) }
            @if merging {
                p { "These versions were edited separately. Compare them, then save one version that combines them. The form starts from the first one." }
                (comparison(item, versions))
                h2 { "The merged version" }
            } @else {
                p.muted { "Saving adds a new version. The current one stays in the history." }
            }
            @if let Some(error) = &form.fields.error { p.error role="alert" { (error) } }
            form method="post" action={ "/item/" (item) "/edit" } enctype="multipart/form-data" {
                @for parent in &form.parents { input type="hidden" name="parent" value=(parent); }
                (pages::guide_fields(&form.fields))
                (auth::csrf_field(&form.fields.csrf))
                @if on_node {
                    p.muted { "This item is kept on this NEVO only, so its new versions stay here too." }
                } @else {
                    label { input type="checkbox" name="node_only" value="1" checked[form.fields.node_only]; " Keep this version on this NEVO only (don't share with other nodes)" }
                }
                @if !choices.is_empty() {
                    fieldset {
                        legend { "Files to keep " span.hint { "(untick a file to remove it)" } }
                        @for (key, f) in &choices {
                            label.check {
                                input type="checkbox" name="keep" value=(key) checked[form.keep.contains(key)];
                                " " (f.name) span.muted { " · " (f.mime) " · " (pages::size(f.size)) }
                            }
                        }
                    }
                }
                // Files come last, as on the add form: the server checks everything typed
                // before it stores an upload.
                label for="files" { "Add files " span.hint { "(optional: PDF, photos, video)" } }
                input type="file" id="files" name="files" multiple;
                button type="submit" { (if merging { "Save the merged version" } else { "Save" }) }
            }
        },
    )
}

/// The versions side by side, then the text of each compared with the first.
fn comparison(item: Id, versions: &[Version]) -> Markup {
    let sides: Vec<Side> = versions.iter().map(Version::side).collect();
    html! {
        div.scroll {
            table.compare {
                tr {
                    th {}
                    @for (i, v) in versions.iter().enumerate() {
                        th {
                            (v.label()) br;
                            a href=(version_url(item, v.id())) { "View" }
                            @if i > 0 {
                                " · " a href={ "/item/" (item) "/merge?start=" (v.id()) } { "Start from this one" }
                            }
                        }
                    }
                }
                (diff::side_by_side(&sides))
            }
        }
        @for v in &versions[1..] {
            h3 { "Text: from " (versions[0].label()) " to " (v.label()) }
            (diff::text(&versions[0].text, &v.text))
        }
    }
}

pub(crate) async fn edit_form(
    State(state): State<AppState>,
    Contributor(session): Contributor,
    Path(id): Path<String>,
) -> AppResult<Html<String>> {
    let item = parse_id(&id)?;
    let csrf = session.csrf_token();
    let page = blocking(move || {
        let store = &state.store;
        let start = visible_tips(store, item)?.remove(0);
        let on_node = stays_on_node(store, item, std::slice::from_ref(&start))?;
        let versions = [load(store, start, &mut HashMap::new())?];
        let mut form = prefill(&versions);
        form.fields.csrf = csrf;
        Ok(edit_page(item, &versions, &form, on_node).into_string())
    })
    .await?;
    Ok(Html(page))
}

pub(crate) async fn merge_form(
    State(state): State<AppState>,
    Contributor(session): Contributor,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> AppResult<Response> {
    let item = parse_id(&id)?;
    let csrf = session.csrf_token();
    let start: Option<Id> = query.get("start").and_then(|s| s.parse().ok());
    let page = blocking(move || {
        let store = &state.store;
        let mut tips = visible_tips(store, item)?;
        if tips.len() < 2 {
            return Ok(None); // nothing to merge (any more)
        }
        tips.truncate(MAX_PARENTS);
        if let Some(pos) = start.and_then(|s| tips.iter().position(|t| t.id() == s)) {
            let tip = tips.remove(pos);
            tips.insert(0, tip);
        }
        let on_node = stays_on_node(store, item, &tips)?;
        let mut texts = HashMap::new();
        let versions = tips
            .into_iter()
            .map(|t| load(store, t, &mut texts))
            .collect::<AppResult<Vec<_>>>()?;
        let mut form = prefill(&versions);
        form.fields.csrf = csrf;
        Ok(Some(edit_page(item, &versions, &form, on_node).into_string()))
    })
    .await?;
    Ok(match page {
        Some(page) => Html(page).into_response(),
        None => Redirect::to(&format!("/item/{item}")).into_response(),
    })
}

pub(crate) async fn edit_submit(
    State(state): State<AppState>,
    Contributor(session): Contributor,
    Path(id): Path<String>,
    multipart: Multipart,
) -> Response {
    let item = match parse_id(&id) {
        Ok(item) => item,
        Err(e) => return e.into_response(),
    };
    let mut form = EditForm::default();
    let result = save(&state, &session, item, multipart, &mut form).await;
    form.fields.csrf = session.csrf_token();
    match result {
        Ok(()) => Redirect::to(&format!("/item/{item}")).into_response(),
        Err(AppError::BadRequest(message)) => {
            form.fields.error = Some(message);
            // Show the form again with what was typed.
            let page = blocking(move || {
                let store = &state.store;
                let parents = parent_revisions(store, item, &form.parents)?;
                let on_node = stays_on_node(store, item, &parents)?;
                let mut texts = HashMap::new();
                let versions = parents
                    .into_iter()
                    .map(|p| load(store, p, &mut texts))
                    .collect::<AppResult<Vec<_>>>()?;
                Ok(edit_page(item, &versions, &form, on_node).into_string())
            })
            .await;
            match page {
                Ok(page) => (StatusCode::BAD_REQUEST, Html(page)).into_response(),
                Err(e) => e.into_response(),
            }
        }
        Err(other) => other.into_response(),
    }
}

/// What a saved edit builds on, checked against the store.
struct Base {
    parents: Vec<Sealed<Revision>>,
    kept: Vec<ContentFile>,
    on_node: bool,
}

async fn resolve(state: &AppState, item: Id, form: &EditForm) -> AppResult<Base> {
    let store = state.store.clone();
    let (ids, keep) = (form.parents.clone(), form.keep.clone());
    blocking(move || {
        let parents = parent_revisions(&store, item, &ids)?;
        let on_node = stays_on_node(&store, item, &parents)?;
        let mut kept = Vec::new();
        let mut seen = HashSet::new();
        for key in &keep {
            let file = key
                .split_once(':')
                .and_then(|(rev, index)| Some((rev.parse::<Id>().ok()?, index.parse::<usize>().ok()?)))
                .and_then(|(rev, index)| parents.iter().find(|p| p.id() == rev)?.object.content.get(index))
                .ok_or_else(|| {
                    AppError::BadRequest("A file to keep isn't part of the versions being edited.".into())
                })?;
            // Kept files reuse their blob, so nothing is uploaded or copied again. The same
            // file in two merged versions is kept once.
            if seen.insert((file.name.clone(), file.blob)) {
                kept.push(file.clone());
            }
        }
        Ok(Base { parents, kept, on_node })
    })
    .await
}

async fn save(
    state: &AppState,
    session: &Session,
    item: Id,
    mut multipart: Multipart,
    form: &mut EditForm,
) -> AppResult<()> {
    let bad = |m: &str| AppError::BadRequest(m.to_string());
    let too_many_files = || AppError::BadRequest(format!("At most {MAX_FILES} files."));
    let mut uploads = Vec::new();
    let mut parts = 0;
    // Loaded before the first upload is stored, or at the end.
    let mut base: Option<Base> = None;
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
            "parent" => {
                if form.parents.len() >= MAX_PARENTS {
                    return Err(AppError::BadRequest(format!(
                        "At most {MAX_PARENTS} versions can be merged at once."
                    )));
                }
                let id = app::text_field(field, MAX_FIELD)
                    .await?
                    .trim()
                    .parse::<Id>()
                    .map_err(|_| bad("This form doesn't match the item any more. Please open it again."))?;
                if !form.parents.contains(&id) {
                    form.parents.push(id);
                }
            }
            "keep" => {
                if form.keep.len() >= MAX_FILES {
                    return Err(too_many_files());
                }
                form.keep.push(app::text_field(field, MAX_FIELD).await?);
            }
            "files" => {
                let Some(file_name) = app::upload_name(&field) else {
                    continue;
                };
                // As in adding: check what was typed before storing anything, so a mistake
                // leaves no uploaded files behind.
                if base.is_none() {
                    auth::check_session_csrf(session, &form.fields.csrf)?;
                    app::check_fields(&form.fields)?;
                    base = Some(resolve(state, item, form).await?);
                }
                let kept = base.as_ref().map_or(0, |b| b.kept.len());
                if kept + uploads.len() >= MAX_FILES {
                    return Err(too_many_files());
                }
                if let Some(file) = app::upload(state, field, file_name).await? {
                    uploads.push(file);
                }
            }
            _ => app::typed_field(&name, field, &mut form.fields).await?,
        }
    }

    auth::check_session_csrf(session, &form.fields.csrf)?;
    app::check_fields(&form.fields)?;
    let base = match base {
        Some(base) => base,
        None => resolve(state, item, form).await?,
    };
    let (topics, text) = app::check_content(&form.fields, !base.kept.is_empty() || !uploads.is_empty())?;
    // Kept files keep their names, so links to them in the text still work. Two files with
    // the same name (from two merged versions) would make those links ambiguous.
    let mut names = HashSet::new();
    let mut files: Vec<ContentFile> = base.kept.into_iter().chain(uploads).collect();
    for f in &mut files {
        f.name = app::unique_name(&f.name, &mut names);
    }

    let first = &base.parents[0].object;
    let guide_name = guide_file(first).map_or_else(|| "guide.md".to_string(), |(_, f)| f.name.clone());
    let has_text = !text.trim().is_empty();
    // The rule adding uses, for the kinds it gives. Other kinds (an imported article, a map)
    // stay what they were.
    let kind = match &first.kind {
        Kind::Guide | Kind::Document | Kind::Video => {
            if has_text {
                Kind::Guide
            } else if files.iter().all(|f| f.mime.starts_with("video/")) {
                Kind::Video
            } else {
                Kind::Document
            }
        }
        other => other.clone(),
    };
    let source = first.source.clone();
    let vis = if base.on_node || form.fields.node_only {
        Visibility::Node
    } else {
        Visibility::Public
    };
    let f = &form.fields;
    let (title, summary, lang, license) = (f.title.clone(), f.summary.clone(), f.lang.clone(), f.license.clone());
    let parents = base.parents;
    let state = state.clone();
    let key = signer(session)?;
    blocking(move || {
        let store = &state.store;
        let mut content = Vec::new();
        if has_text {
            let info = store.put_blob(text.as_bytes())?;
            content.push(ContentFile {
                name: app::unique_name(&guide_name, &mut names),
                blob: info.id,
                size: info.size,
                mime: "text/markdown".into(),
            });
        }
        content.extend(files);
        let author = key.public();
        let _adding = state.adding.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // One more than the author's last object and every version this builds on (spec
        // section 2), so the new version sorts after all of them.
        let parents_clock = parents.iter().map(|p| p.object.clock).max().unwrap_or(0);
        let revision = Revision {
            item: Some(item),
            parents: parents.iter().map(Sealed::id).collect(),
            author,
            clock: store.author_clock(author)?.max(parents_clock) + 1,
            time: app::now(),
            kind,
            title,
            lang: if lang.is_empty() { "en".into() } else { lang },
            topics,
            license: if license.is_empty() {
                "CC-BY-SA-4.0".into()
            } else {
                license
            },
            vis,
            content,
            source,
            summary: (!summary.is_empty()).then_some(summary),
            extra: Default::default(),
        };
        let sealed = seal(&revision, &key).map_err(|e| AppError::BadRequest(format!("Please check the form: {e}")))?;
        if vis == Visibility::Public {
            store.publish(&state.node, sealed.bytes())?;
        } else {
            store.put_object(sealed.bytes())?;
        }
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// History and old versions

pub(crate) async fn history(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> AppResult<Html<String>> {
    let item = parse_id(&id)?;
    let page: usize = query.get("page").and_then(|p| p.parse().ok()).unwrap_or(0);
    let html = blocking(move || {
        let store = &state.store;
        let all: Vec<_> = store
            .item_revisions(item)?
            .into_iter()
            .filter(|r| r.object.vis != Visibility::Private)
            .collect();
        let tips: Vec<Id> = visible_tips(store, item)?.iter().map(Sealed::id).collect();
        let by_id: HashMap<Id, &Sealed<Revision>> = all.iter().map(|r| (r.id(), r)).collect();
        // Newest first.
        let shown: Vec<&Sealed<Revision>> = all
            .iter()
            .rev()
            .skip(page.saturating_mul(HISTORY_PAGE))
            .take(HISTORY_PAGE)
            .collect();
        if shown.is_empty() {
            return Err(AppError::NotFound);
        }
        let mut texts = HashMap::new();
        let mut loaded: HashMap<Id, Version> = HashMap::new();
        for r in &shown {
            for id in std::iter::once(r.id()).chain(r.object.parents.iter().copied()) {
                if let (false, Some(rev)) = (loaded.contains_key(&id), by_id.get(&id)) {
                    loaded.insert(id, load(store, (*rev).clone(), &mut texts)?);
                }
            }
        }
        let view = HistoryView {
            item,
            title: &by_id[&tips[0]].object.title,
            page,
            total: all.len(),
            shown: &shown,
            loaded: &loaded,
            tips: &tips,
        };
        Ok(history_page(&view).into_string())
    })
    .await?;
    Ok(Html(html))
}

/// What one history page shows.
struct HistoryView<'a> {
    item: Id,
    /// The current title.
    title: &'a str,
    page: usize,
    /// Versions in the whole history.
    total: usize,
    /// This page's versions, newest first.
    shown: &'a [&'a Sealed<Revision>],
    /// The shown versions and their parents, with their texts.
    loaded: &'a HashMap<Id, Version>,
    tips: &'a [Id],
}

fn history_page(view: &HistoryView) -> Markup {
    let HistoryView {
        item,
        title,
        page,
        total,
        shown,
        loaded,
        tips,
    } = *view;
    let more = (page + 1) * HISTORY_PAGE < total;
    pages::layout(
        "History",
        html! {
            style { (PreEscaped(CSS)) }
            p.muted { a href={ "/item/" (item) } { (title) } " / History" }
            h1 { "History" }
            p.muted { (total) (if total == 1 { " version" } else { " versions" }) ". Edits never replace a version: every one is kept." }
            ul.items {
                @for r in shown {
                    @let v = &loaded[&r.id()];
                    @let parents = &r.object.parents;
                    li {
                        strong { (v.label()) }
                        @if tips.contains(&r.id()) { " " span.tag { "current" } }
                        div { (r.object.title) }
                        @if parents.len() > 1 { div.muted { "Merges " (parents.len()) " versions." } }
                        p { a href=(version_url(item, r.id())) { "View this version" } }
                        @if parents.is_empty() { p.muted { "The first version." } }
                        @for parent in parents {
                            @if let Some(p) = loaded.get(parent) {
                                details open[parents.len() == 1] {
                                    summary { "Changes from version " (p.rev.object.clock) }
                                    (diff::changes(&p.side(), &v.side()))
                                }
                            } @else {
                                p.muted { "The version before it isn't on this NEVO." }
                            }
                        }
                    }
                }
            }
            p {
                @if page > 0 { a href={ "/item/" (item) "/history?page=" (page - 1) } { "Newer versions" } " " }
                @if more { a href={ "/item/" (item) "/history?page=" (page + 1) } { "Older versions" } }
            }
        },
    )
}

/// One version of an item, as it was.
pub(crate) async fn version(
    State(state): State<AppState>,
    Path((id, rev)): Path<(String, String)>,
) -> AppResult<Html<String>> {
    let (item, rev) = (parse_id(&id)?, parse_id(&rev)?);
    let html = blocking(move || {
        let store = &state.store;
        let r = match store.get::<Revision>(rev) {
            Ok(Some(r)) => r,
            Ok(None) | Err(StoreError::Format(_)) => return Err(AppError::NotFound),
            Err(e) => return Err(e.into()),
        };
        if r.object.item_id(r.id()) != item || r.object.vis == Visibility::Private {
            return Err(AppError::NotFound);
        }
        let current = visible_tips(store, item)?.iter().any(|t| t.id() == rev);
        Ok(version_page(item, &load(store, r, &mut HashMap::new())?, current).into_string())
    })
    .await?;
    Ok(Html(html))
}

fn version_page(item: Id, v: &Version, current: bool) -> Markup {
    let r = &v.rev.object;
    let rev = v.id();
    let guide = v.guide.map(|_| {
        markdown::render(&v.text, |name| {
            r.content
                .iter()
                .position(|c| c.name == name)
                .map(|i| pages::file_url(rev, i, &r.content[i]))
        })
    });
    pages::layout(
        &r.title,
        html! {
            p.muted { a href={ "/item/" (item) } { "Current version" } " / " a href={ "/item/" (item) "/history" } { "History" } }
            p.notice {
                "You are looking at " (v.label()) ". "
                (if current { "It is a current version." } else { "A newer version exists." })
            }
            p.muted { @for t in &r.topics { a.tag href={ "/topic/" (t) } { (t) } } }
            h1 { (r.title) }
            @if let Some(summary) = &r.summary { p { (summary) } }
            p.meta {
                span { "Language: " (r.lang) }
                span { "License: " (r.license) }
                @if r.vis == Visibility::Node { span { "Only on this NEVO" } }
            }
            @if let Some(guide) = &guide { div.guide { (PreEscaped(guide)) } }
            h2 { "Files" }
            ul.items {
                @for (i, f) in r.content.iter().enumerate() {
                    li {
                        a href=(pages::file_url(rev, i, f)) { (f.name) }
                        span.muted { " · " (f.mime) " · " (pages::size(f.size)) }
                    }
                }
            }
        },
    )
}
