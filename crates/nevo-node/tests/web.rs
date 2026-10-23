//! Drives the real router: the same code paths a browser uses.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use nevo_format::cbor::Map;
use nevo_format::*;
use nevo_node::accounts::{Accounts, Role, ACCOUNTS_FILE};
use nevo_node::auth::SESSION_COOKIE;
use nevo_node::{router, AppState};
use nevo_store::Store;
use tower::util::MapRequestLayer;
use tower::ServiceExt;

struct Node {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    key: Arc<Keypair>,
    app: Router,
}

/// A node that is set up, with every request sent by its signed-in admin from one of its own
/// pages. tests/accounts.rs covers readers, sign-in and forged forms.
fn node() -> Node {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let key = Arc::new(Keypair::from_secret([7; 32]));
    let accounts = Arc::new(Accounts::open(dir.path().join(ACCOUNTS_FILE)).unwrap());
    // Publishes the admin's key record: the first entry of the node's log.
    accounts
        .create(&store, &key, "Tester", "test passphrase", Role::Admin, true)
        .unwrap();
    let session = accounts.sign_in("Tester", "test passphrase").unwrap();
    let cookie = format!("{SESSION_COOKIE}={}", session.cookie);
    let app = router(AppState::new(store.clone(), key.clone(), accounts)).layer(MapRequestLayer::new(
        move |mut request: Request<Body>| {
            let headers = request.headers_mut();
            headers.insert(header::COOKIE, cookie.parse().unwrap());
            headers.insert(header::HOST, "nevo.test".parse().unwrap());
            headers.insert(header::ORIGIN, "http://nevo.test".parse().unwrap());
            request
        },
    ));
    Node {
        _dir: dir,
        store,
        key,
        app,
    }
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn header(&self, name: header::HeaderName) -> String {
        self.headers
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }
}

async fn send(app: &Router, request: Request<Body>) -> Reply {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes().to_vec();
    Reply { status, headers, body }
}

async fn get(app: &Router, uri: &str) -> Reply {
    send(app, Request::get(uri).body(Body::empty()).unwrap()).await
}

/// One multipart form part: (field name, optional (file name, content type), data).
type Part<'a> = (&'a str, Option<(&'a str, &'a str)>, &'a [u8]);

async fn post_form(app: &Router, parts: &[Part<'_>]) -> Reply {
    // The form carries the session's form token (auth.rs); take it from the page, as a browser does.
    let page = get(app, "/add").await.text();
    let csrf = page
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let mut with_token: Vec<Part> = vec![("csrf", None, csrf.as_bytes())];
    with_token.extend_from_slice(parts);
    let boundary = "nevo-test-boundary-7d3f";
    let mut body = Vec::new();
    for (name, file, data) in &with_token {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some((file_name, mime)) => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes(),
            ),
            None => body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes()),
        }
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let request = Request::post("/add")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    send(app, request).await
}

/// Adds a burns guide with a photo and returns the item page's URL.
async fn add_burns_guide(app: &Router, extra: &[Part<'_>]) -> String {
    let text = b"1. Cool the burn under clean running water for 20 minutes.\n\n**Do not** put butter on it.\n\n<script>alert('x')</script>\n\n![The burn](photo.jpg)\n\n[evil](javascript:alert(1))";
    let mut parts: Vec<Part> = vec![
        ("title", None, b"Treating a small burn"),
        ("summary", None, b"First aid for small kitchen burns."),
        ("topics", None, b"medical/first-aid"),
        ("text", None, text),
        ("files", Some(("photo.jpg", "image/jpeg")), b"\xff\xd8\xff fake jpeg"),
        ("lang", None, b"en"),
        ("license", None, b"CC-BY-SA-4.0"),
    ];
    parts.extend_from_slice(extra);
    let reply = post_form(app, &parts).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    reply.header(header::LOCATION)
}

#[tokio::test]
async fn pages_load_with_security_headers() {
    let n = node();
    let home = get(&n.app, "/").await;
    assert_eq!(home.status, StatusCode::OK);
    assert!(home.text().contains("Nothing here yet"));
    assert!(home
        .header(header::CONTENT_SECURITY_POLICY)
        .contains("default-src 'none'"));
    assert_eq!(home.header(header::X_CONTENT_TYPE_OPTIONS), "nosniff");
    assert!(!home.text().contains("<script"), "pages need no JavaScript");

    assert_eq!(get(&n.app, "/topics").await.status, StatusCode::OK);
    assert_eq!(get(&n.app, "/add").await.status, StatusCode::OK);
    assert_eq!(get(&n.app, "/health").await.text(), "ok");
    assert_eq!(get(&n.app, "/item/nonsense").await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        get(&n.app, "/topic/NOT%20a%20topic").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn add_a_guide_and_read_it() {
    let n = node();
    let url = add_burns_guide(&n.app, &[]).await;
    assert!(url.starts_with("/item/bdyq"));

    let page = get(&n.app, &url).await;
    assert_eq!(page.status, StatusCode::OK);
    let html = page.text();
    assert!(html.contains("Treating a small burn"));
    assert!(html.contains("<strong>Do not</strong>"));
    assert!(html.contains("&lt;script&gt;"), "raw HTML in a guide is shown as text");
    assert!(!html.contains("<script>alert"));
    assert!(!html.contains("javascript:"));
    assert!(html.contains("Added on this NEVO"));
    // The Markdown image points at the uploaded photo.
    assert!(html.contains("/1/photo.jpg\""), "{html}");

    // It is listed on the home page and under its topic and parent topic, and was published.
    assert!(get(&n.app, "/").await.text().contains("Treating a small burn"));
    assert!(get(&n.app, "/topic/medical")
        .await
        .text()
        .contains("Treating a small burn"));
    assert!(get(&n.app, "/topic/medical/first-aid")
        .await
        .text()
        .contains("Treating a small burn"));
    assert!(!get(&n.app, "/topic/water")
        .await
        .text()
        .contains("Treating a small burn"));
    // After the admin's key record.
    assert_eq!(n.store.log_position(n.key.public()).unwrap(), 2);
}

#[tokio::test]
async fn files_are_served_safely() {
    let n = node();
    let url = add_burns_guide(
        &n.app,
        &[("files", Some(("page.html", "text/html")), b"<script>steal()</script>")],
    )
    .await;
    let item: Id = url.trim_start_matches("/item/").parse().unwrap();
    let rev = n.store.item_tips(item).unwrap()[0].id();

    let guide = get(&n.app, &format!("/file/{rev}/0/guide.md")).await;
    assert_eq!(guide.status, StatusCode::OK);
    assert_eq!(guide.header(header::CONTENT_TYPE), "text/plain; charset=utf-8");
    assert!(guide.text().starts_with("1. Cool the burn"));

    let photo = get(&n.app, &format!("/file/{rev}/1/photo.jpg")).await;
    assert_eq!(photo.header(header::CONTENT_TYPE), "image/jpeg");
    assert!(photo.header(header::CONTENT_DISPOSITION).starts_with("inline"));

    // HTML is downloaded, never displayed.
    let page = get(&n.app, &format!("/file/{rev}/2/page.html")).await;
    assert_eq!(page.header(header::CONTENT_TYPE), "application/octet-stream");
    assert!(page.header(header::CONTENT_DISPOSITION).starts_with("attachment"));

    // Range requests let video and audio seek.
    let range = send(
        &n.app,
        Request::get(format!("/file/{rev}/0/guide.md"))
            .header(header::RANGE, "bytes=0-3")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(range.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(range.body, b"1. C");

    assert_eq!(
        get(&n.app, &format!("/file/{rev}/9/x")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&n.app, &format!("/file/{}/0/x", Id::of(b"nope"))).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn node_only_guides_stay_out_of_the_log() {
    let n = node();
    let url = add_burns_guide(&n.app, &[("node_only", None, b"1")]).await;
    let page = get(&n.app, &url).await.text();
    assert!(page.contains("Only on this NEVO"));
    assert_eq!(n.store.log_position(n.key.public()).unwrap(), 1, "only the key record");
}

#[tokio::test]
async fn private_items_are_hidden() {
    let n = node();
    let author = Keypair::from_secret([1; 32]);
    let blob = n.store.put_blob(&b"secret notes"[..]).unwrap();
    let private = seal(
        &Revision {
            item: None,
            parents: vec![],
            author: author.public(),
            clock: 1,
            time: Time {
                ms: 0,
                src: TimeSource::Unknown,
            },
            kind: Kind::Guide,
            title: "My diary".into(),
            lang: "en".into(),
            topics: vec!["general".parse().unwrap()],
            license: "CC-BY-SA-4.0".into(),
            vis: Visibility::Private,
            content: vec![ContentFile {
                name: "d.md".into(),
                blob: blob.id,
                size: blob.size,
                mime: "text/markdown".into(),
            }],
            source: None,
            summary: None,
            extra: Map::new(),
        },
        &author,
    )
    .unwrap();
    n.store.put_object(private.bytes()).unwrap();

    assert_eq!(
        get(&n.app, &format!("/item/{}", private.id())).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&n.app, &format!("/file/{}/0/d.md", private.id())).await.status,
        StatusCode::NOT_FOUND
    );
    assert!(!get(&n.app, "/").await.text().contains("My diary"));
    assert!(!get(&n.app, "/topic/general").await.text().contains("My diary"));
}

#[tokio::test]
async fn form_mistakes_keep_what_was_typed() {
    let n = node();
    let reply = post_form(
        &n.app,
        &[
            ("title", None, b""),
            ("text", None, b"My careful steps"),
            ("topics", None, b"water"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.text().contains("Please give it a title."));
    assert!(reply.text().contains("My careful steps"));

    let reply = post_form(&n.app, &[("title", None, b"Wells"), ("topics", None, b"water")]).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.text().contains("Write some text or attach at least one file."));

    let reply = post_form(
        &n.app,
        &[
            ("title", None, b"Wells"),
            ("text", None, b"Dig"),
            ("topics", None, b"BAD TOPIC"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(n.store.log_position(n.key.public()).unwrap(), 1, "only the key record");
}

/// Counts files in the blob store folder, to check that rejected forms keep nothing.
fn blob_files(n: &Node) -> usize {
    fn count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| if e.path().is_dir() { count(&e.path()) } else { 1 })
                    .sum()
            })
            .unwrap_or(0)
    }
    count(&n.store.root().join("blobs"))
}

#[tokio::test]
async fn forms_are_limited() {
    let n = node();
    let topic: Part = ("topics", None, b"water");
    let mut parts: Vec<Part> = vec![("title", None, b"Wells"), ("text", None, b"Dig")];
    parts.extend(std::iter::repeat_n(topic, 17));
    let reply = post_form(&n.app, &parts).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.text().contains("At most 16 topics."));

    let junk: Part = ("other", None, b"x");
    let reply = post_form(&n.app, &vec![junk; 201]).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.text().contains("too many parts"));

    let long_title = vec![b'a'; 501];
    let reply = post_form(
        &n.app,
        &[
            ("title", None, &long_title),
            ("topics", None, b"water"),
            ("text", None, b"x"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_rejected_form_stores_no_files() {
    let n = node();
    let reply = post_form(
        &n.app,
        &[
            ("topics", None, b"water"),
            ("files", Some(("big.pdf", "application/pdf")), b"%PDF-1.7 pretend"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.text().contains("Please give it a title."));
    assert_eq!(blob_files(&n), 0, "the upload was refused before it was stored");
}

#[tokio::test]
async fn a_full_disk_refuses_uploads_politely() {
    let n = node();
    assert!(get(&n.app, "/").await.text().contains(" free"), "home shows free space");

    n.store.set_reserve(u64::MAX);
    let reply = post_form(
        &n.app,
        &[
            ("title", None, b"Wells"),
            ("topics", None, b"water"),
            ("text", None, b"My careful steps"),
            ("files", Some(("well.pdf", "application/pdf")), b"%PDF-1.7 pretend"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::INSUFFICIENT_STORAGE);
    assert!(reply.text().contains("almost out of storage space"));
    assert!(
        reply.text().contains("My careful steps"),
        "the form keeps what was typed"
    );
    assert_eq!(blob_files(&n), 0);

    // Text alone is stored as a blob too, so it is refused the same way.
    let reply = post_form(
        &n.app,
        &[
            ("title", None, b"Wells"),
            ("topics", None, b"water"),
            ("text", None, b"My careful steps"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::INSUFFICIENT_STORAGE);
    assert!(reply.text().contains("My careful steps"));
    // Only the admin's key record, from setup, is in the log.
    assert_eq!(n.store.log_position(n.key.public()).unwrap(), 1);
}

#[tokio::test]
async fn same_named_files_are_numbered() {
    let n = node();
    let url = add_burns_guide(
        &n.app,
        &[
            ("files", Some(("guide.md", "text/markdown")), b"# notes"),
            ("files", Some(("photo.jpg", "image/jpeg")), b"\xff\xd8 second"),
        ],
    )
    .await;
    let item: Id = url.trim_start_matches("/item/").parse().unwrap();
    let names: Vec<String> = n.store.item_tips(item).unwrap()[0]
        .object
        .content
        .iter()
        .map(|f| f.name.clone())
        .collect();
    assert_eq!(names, vec!["guide (2).md", "photo.jpg", "guide.md", "photo (2).jpg"]);
}

#[tokio::test]
async fn odd_requests_get_clean_answers() {
    let n = node();
    let url = add_burns_guide(&n.app, &[]).await;
    let item: Id = url.trim_start_matches("/item/").parse().unwrap();
    let rev = n.store.item_tips(item).unwrap()[0].id();
    // The ID of a log entry is not a revision: 404, not an error page.
    let (entry, _) = n.store.log_entries(n.key.public(), 0, 1).unwrap().remove(0);
    assert_eq!(
        get(&n.app, &format!("/file/{}/0/x", Id::of(&entry))).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&n.app, &format!("/file/{rev}/notanumber/x")).await.status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn text_that_is_not_utf8_still_shows() {
    let n = node();
    // "Café" in Windows-1252, attached as the guide text.
    let reply = post_form(
        &n.app,
        &[
            ("title", None, b"Old notes"),
            ("topics", None, b"general"),
            ("files", Some(("notes.md", "text/markdown")), b"Caf\xe9 **menu**"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let page = get(&n.app, &reply.header(header::LOCATION)).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.text().contains("<strong>menu</strong>"));
}

#[tokio::test]
async fn simultaneous_additions_get_distinct_clocks() {
    let n = node();
    let adds = (0..8).map(|i| {
        let app = n.app.clone();
        tokio::spawn(async move {
            let title = format!("Guide {i}");
            post_form(
                &app,
                &[
                    ("title", None, title.as_bytes()),
                    ("topics", None, b"general"),
                    ("text", None, b"Steps"),
                ],
            )
            .await
        })
    });
    for add in adds {
        assert_eq!(add.await.unwrap().status, StatusCode::SEE_OTHER);
    }
    let mut clocks: Vec<u64> = n
        .store
        .recent_tips(100)
        .unwrap()
        .iter()
        .map(|r| r.object.clock)
        .collect();
    clocks.sort();
    // The author's key record has clock 1.
    assert_eq!(clocks, (2..=9).collect::<Vec<_>>());
}

/// Search is updated in the background after a change, so poll briefly.
async fn search_until(app: &Router, query: &str, want: &str) -> String {
    for _ in 0..50 {
        let page = get(app, &format!("/search?q={query}")).await.text();
        if page.contains(want) {
            return page;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    get(app, &format!("/search?q={query}")).await.text()
}

#[tokio::test]
async fn added_guides_can_be_searched() {
    let n = node();
    assert!(
        get(&n.app, "/").await.text().contains("action=\"/search\""),
        "home has a search box"
    );
    add_burns_guide(&n.app, &[]).await;
    let page = search_until(&n.app, "running%20water", "Treating a small burn").await;
    assert!(page.contains("Treating a small burn"), "{page}");
    assert!(page.contains("<b>"), "matches are highlighted");
    assert!(!page.contains("<script>alert"), "guide text in snippets is escaped");
    let empty = get(&n.app, "/search?q=zzzqqqxxx").await.text();
    assert!(empty.contains("Nothing in the library matches"));
}
