//! Editing, merging and history, driven through the real router.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
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

fn node() -> Node {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let key = Arc::new(Keypair::from_secret([7; 32]));
    // Editing needs a signed-in contributor (accounts.rs): create one and send its cookie, plus
    // the Host and Origin a browser sends, which the forged-form check compares.
    let accounts = Arc::new(Accounts::open(dir.path().join(ACCOUNTS_FILE)).unwrap());
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
    fn location(&self) -> String {
        self.headers
            .get(header::LOCATION)
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

async fn post(app: &Router, uri: &str, parts: &[Part<'_>]) -> Reply {
    // The form carries the session's form token (auth.rs); take it from a page, as a browser does.
    let page = get(app, "/add").await.text();
    let csrf = page
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
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
    let request = Request::post(uri)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    send(app, request).await
}

const GUIDE: &[u8] =
    b"1. Cool the burn under running water.\n\n<script>alert('x')</script>\n\n![The burn](photo.jpg)\n";

/// Adds a burns guide (guide text, then a photo) and returns its item ID.
async fn add_guide(n: &Node, extra: &[Part<'_>]) -> Id {
    let mut parts: Vec<Part> = vec![
        ("title", None, b"Treating a small burn"),
        ("topics", None, b"medical/first-aid"),
        ("text", None, GUIDE),
        ("files", Some(("photo.jpg", "image/jpeg")), b"\xff\xd8\xff fake jpeg"),
    ];
    parts.extend_from_slice(extra);
    let reply = post(&n.app, "/add", &parts).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    reply.location().trim_start_matches("/item/").parse().unwrap()
}

fn tips(n: &Node, item: Id) -> Vec<Sealed<Revision>> {
    n.store.item_tips(item).unwrap()
}

fn keep(rev: Id, index: usize) -> Vec<u8> {
    format!("{rev}:{index}").into_bytes()
}

/// Saves an edit of `parents` with the given title and text, keeping the listed files.
async fn edit(n: &Node, item: Id, parents: &[Id], title: &str, text: &[u8], kept: &[Vec<u8>]) -> Reply {
    let parent_ids: Vec<Vec<u8>> = parents.iter().map(|p| p.to_string().into_bytes()).collect();
    let mut parts: Vec<Part> = parent_ids.iter().map(|p| ("parent", None, p.as_slice())).collect();
    parts.extend([
        ("title", None, title.as_bytes()),
        ("topics", None, b"medical/first-aid".as_slice()),
        ("text", None, text),
    ]);
    parts.extend(kept.iter().map(|k| ("keep", None, k.as_slice())));
    post(&n.app, &format!("/item/{item}/edit"), &parts).await
}

/// Counts files in the blob store folder.
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
async fn an_edit_is_a_child_version() {
    let n = node();
    let item = add_guide(&n, &[]).await;
    let first = tips(&n, item).remove(0);

    // The form starts from the current version.
    let form = get(&n.app, &format!("/item/{item}/edit")).await;
    assert_eq!(form.status, StatusCode::OK);
    let html = form.text();
    assert!(html.contains("value=\"Treating a small burn\""));
    assert!(html.contains("1. Cool the burn under running water."));
    assert!(html.contains(&format!("name=\"parent\" value=\"{}\"", first.id())));
    assert!(
        html.contains(&format!("value=\"{}:1\" checked", first.id())),
        "the photo is kept"
    );
    assert!(
        !html.contains(&format!("value=\"{}:0\"", first.id())),
        "the guide text is edited, not kept"
    );

    let reply = edit(
        &n,
        item,
        &[first.id()],
        "Treating a small burn safely",
        b"1. Cool the burn for 20 minutes.\r\n\r\n![The burn](photo.jpg)\r\n",
        &[keep(first.id(), 1)],
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    assert_eq!(reply.location(), format!("/item/{item}"));

    let now = tips(&n, item);
    assert_eq!(now.len(), 1);
    let r = &now[0].object;
    assert_eq!(r.item, Some(item));
    assert_eq!(r.parents, vec![first.id()]);
    assert_eq!(r.clock, 3, "the contributor's key record has clock 1");
    assert_ne!(
        r.author,
        n.key.public(),
        "signed by the contributor's own key, not the node key"
    );
    assert_eq!(r.vis, Visibility::Public);
    assert_eq!(r.kind, Kind::Guide);
    let names: Vec<&str> = r.content.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["guide.md", "photo.jpg"]);
    assert_eq!(r.content[1], first.object.content[1]);
    // Line endings from the browser are made plain, as when adding.
    let stored = "1. Cool the burn for 20 minutes.\n\n![The burn](photo.jpg)\n";
    assert_eq!(r.content[0].size, stored.len() as u64);
    assert_eq!(
        n.store.log_position(n.key.public()).unwrap(),
        3,
        "the admin's key record, the first version and the edit were published"
    );

    let page = get(&n.app, &format!("/item/{item}")).await.text();
    assert!(page.contains("Treating a small burn safely"));
    assert!(page.contains("Cool the burn for 20 minutes."));
    assert!(page.contains(&format!("/item/{item}/edit")));
    assert!(!page.contains("Compare and merge"));
}

#[tokio::test]
async fn the_clock_passes_every_parent() {
    let n = node();
    let item = add_guide(&n, &[]).await;
    let first = tips(&n, item).remove(0);
    // Someone else's edit with a much higher clock arrived from another node.
    let other = Keypair::from_secret([3; 32]);
    let mut theirs = first.object.clone();
    theirs.item = Some(item);
    theirs.parents = vec![first.id()];
    theirs.author = other.public();
    theirs.clock = 50;
    theirs.title = "Their edit".into();
    let theirs = seal(&theirs, &other).unwrap();
    n.store.put_object(theirs.bytes()).unwrap();

    let reply = edit(&n, item, &[theirs.id()], "My edit", b"Steps", &[]).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    let tip = tips(&n, item).remove(0);
    assert_eq!(tip.object.parents, vec![theirs.id()]);
    assert_eq!(tip.object.clock, 51);
}

#[tokio::test]
async fn concurrent_edits_branch_and_a_merge_joins_them() {
    let n = node();
    let item = add_guide(&n, &[]).await;
    let first = tips(&n, item).remove(0);

    // Two people opened the same version and both saved.
    let photo = [keep(first.id(), 1)];
    let a = edit(
        &n,
        item,
        &[first.id()],
        "Burns: version A",
        b"Step one\nStep A\n",
        &photo,
    )
    .await;
    let b = edit(&n, item, &[first.id()], "Burns: version B", b"Step one\nStep B\n", &[]).await;
    assert_eq!(a.status, StatusCode::SEE_OTHER);
    assert_eq!(
        b.status,
        StatusCode::SEE_OTHER,
        "the second save still works: it starts a branch"
    );
    let branches = tips(&n, item);
    assert_eq!(branches.len(), 2);
    for tip in &branches {
        assert_eq!(tip.object.parents, vec![first.id()]);
    }

    let page = get(&n.app, &format!("/item/{item}")).await.text();
    assert!(page.contains(&format!("href=\"/item/{item}/merge\"")));
    assert!(page.contains("Compare and merge"));

    // The merge page shows both versions and how they differ.
    let merge = get(&n.app, &format!("/item/{item}/merge")).await;
    assert_eq!(merge.status, StatusCode::OK);
    let html = merge.text();
    assert!(html.contains("Burns: version A") && html.contains("Burns: version B"));
    assert!(html.contains("Step A") && html.contains("Step B"));
    assert!(html.contains("class=\"del\""), "{html}");
    for tip in &branches {
        assert!(html.contains(&format!("name=\"parent\" value=\"{}\"", tip.id())));
    }

    let ids: Vec<Id> = branches.iter().map(|t| t.id()).collect();
    let with_photo = branches.iter().find(|t| t.object.content.len() == 2).unwrap();
    let reply = edit(
        &n,
        item,
        &ids,
        "Burns: merged",
        b"Step one\nStep A\nStep B\n",
        &[keep(with_photo.id(), 1)],
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());

    let merged = tips(&n, item);
    assert_eq!(merged.len(), 1);
    let r = &merged[0].object;
    assert_eq!(r.parents, ids, "every tip is a parent");
    assert_eq!(r.clock, 5, "the contributor's key record has clock 1");
    assert_eq!(r.title, "Burns: merged");
    assert_eq!(r.content[1].blob, first.object.content[1].blob);
    assert!(!get(&n.app, &format!("/item/{item}"))
        .await
        .text()
        .contains("Compare and merge"));
    // Nothing is left to merge.
    assert_eq!(
        get(&n.app, &format!("/item/{item}/merge")).await.status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn history_and_diffs_escape_html() {
    let n = node();
    let item = add_guide(&n, &[]).await;
    let first = tips(&n, item).remove(0);
    let text = b"1. Cool it.\n\n<img src=x onerror=alert(2)>\n";
    let reply = edit(&n, item, &[first.id()], "<i>Burns</i>", text, &[]).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    let second = tips(&n, item).remove(0);
    // A second branch, so the merge page has something to compare.
    let reply = edit(
        &n,
        item,
        &[first.id()],
        "<b>Burns</b>",
        b"<script>alert(3)</script>\n",
        &[],
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());

    let history = get(&n.app, &format!("/item/{item}/history")).await;
    assert_eq!(history.status, StatusCode::OK);
    let merge = get(&n.app, &format!("/item/{item}/merge")).await;
    assert_eq!(merge.status, StatusCode::OK);
    let old = get(&n.app, &format!("/item/{item}/version/{}", first.id())).await;
    assert_eq!(old.status, StatusCode::OK);
    for page in [history.text(), merge.text(), old.text()] {
        assert!(!page.contains("<script>alert"), "{page}");
        assert!(!page.contains("<img src=x"), "{page}");
        assert!(!page.contains("<i>Burns"), "{page}");
        assert!(!page.contains("<b>Burns"), "{page}");
    }
    let history = history.text();
    assert!(history.contains("&lt;img src=x onerror=alert(2)&gt;"));
    assert!(history.contains("&lt;i&gt;Burns&lt;/i&gt;"));
    assert!(history.contains("&lt;script&gt;alert('x')&lt;/script&gt;"));
    assert!(history.contains(&format!("/item/{item}/version/{}", first.id())));
    assert!(history.contains(&format!("/item/{item}/version/{}", second.id())));
    assert!(history.contains("The first version."));
    assert!(merge.text().contains("&lt;script&gt;alert(3)&lt;/script&gt;"));
    assert!(old.text().contains("A newer version exists."));

    // Versions of other items, and unknown IDs, are not found.
    let other = add_guide(&n, &[]).await;
    assert_eq!(
        get(&n.app, &format!("/item/{other}/version/{}", first.id()))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&n.app, &format!("/item/{}/history", Id::of(b"nothing")))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn kept_files_are_not_uploaded_again() {
    let n = node();
    let item = add_guide(&n, &[]).await;
    let first = tips(&n, item).remove(0);
    let before = blob_files(&n);

    // Same text, new title, photo kept: no blob is added.
    let reply = edit(&n, item, &[first.id()], "Burns", GUIDE, &[keep(first.id(), 1)]).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    assert_eq!(blob_files(&n), before);
    let second = tips(&n, item).remove(0);
    assert_eq!(second.object.content, first.object.content);

    // Unticking the photo removes it from the new version only.
    let reply = edit(&n, item, &[second.id()], "Burns", GUIDE, &[]).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let third = tips(&n, item).remove(0);
    assert_eq!(third.object.content.len(), 1);
    assert_eq!(blob_files(&n), before, "older versions still use the photo");

    // A new upload is stored alongside a kept file.
    let reply = post(
        &n.app,
        &format!("/item/{item}/edit"),
        &[
            ("parent", None, second.id().to_string().as_bytes()),
            ("title", None, b"Burns"),
            ("topics", None, b"medical"),
            ("text", None, b""),
            ("keep", None, &keep(second.id(), 1)),
            ("files", Some(("chart.pdf", "application/pdf")), b"%PDF-1.7 chart"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    let r = tips(&n, item)
        .into_iter()
        .find(|t| t.object.content.len() == 2 && t.object.content[1].name == "chart.pdf")
        .expect("the new version");
    assert_eq!(r.object.content[0], first.object.content[1]);
    assert_eq!(r.object.kind, Kind::Document, "no text left");
    assert_eq!(blob_files(&n), before + 1);
}

#[tokio::test]
async fn node_only_items_stay_node_only() {
    let n = node();
    let item = add_guide(&n, &[("node_only", None, b"1")]).await;
    let first = tips(&n, item).remove(0);
    assert_eq!(first.object.vis, Visibility::Node);

    let form = get(&n.app, &format!("/item/{item}/edit")).await.text();
    assert!(form.contains("kept on this NEVO only"));
    assert!(!form.contains("name=\"node_only\""));

    // The form has no node_only field, and the edit still stays here.
    let reply = edit(&n, item, &[first.id()], "Burns", b"Steps", &[]).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    let tip = tips(&n, item).remove(0);
    assert_eq!(tip.object.parents, vec![first.id()]);
    assert_eq!(tip.object.vis, Visibility::Node);
    assert_eq!(
        n.store.log_position(n.key.public()).unwrap(),
        1,
        "nothing was published but the admin's key record"
    );

    // A public item can be kept here from some version on.
    let public = add_guide(&n, &[]).await;
    let first = tips(&n, public).remove(0);
    let parent = first.id().to_string();
    let parts: Vec<Part> = vec![
        ("parent", None, parent.as_bytes()),
        ("title", None, b"Burns"),
        ("topics", None, b"medical"),
        ("text", None, b"Steps"),
        ("node_only", None, b"1"),
    ];
    let reply = post(&n.app, &format!("/item/{public}/edit"), &parts).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    assert_eq!(tips(&n, public)[0].object.vis, Visibility::Node);
    assert_eq!(
        n.store.log_position(n.key.public()).unwrap(),
        2,
        "only the admin's key record and the first version"
    );
}

#[tokio::test]
async fn edit_mistakes_keep_what_was_typed() {
    let n = node();
    let item = add_guide(&n, &[]).await;
    let first = tips(&n, item).remove(0);
    let blobs = blob_files(&n);

    // The same checks as adding, before any upload is stored.
    let reply = post(
        &n.app,
        &format!("/item/{item}/edit"),
        &[
            ("parent", None, first.id().to_string().as_bytes()),
            ("title", None, b""),
            ("topics", None, b"medical"),
            ("text", None, b"My careful new steps"),
            ("files", Some(("big.pdf", "application/pdf")), b"%PDF-1.7 pretend"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.text().contains("Please give it a title."));
    assert!(reply.text().contains("My careful new steps"));
    assert_eq!(blob_files(&n), blobs);

    let reply = edit(&n, item, &[first.id()], "Burns", b"", &[]).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.text().contains("Write some text or attach at least one file."));

    // A version of another item, or a file from a version not being edited, is refused.
    let other = add_guide(&n, &[]).await;
    let other_rev = tips(&n, other).remove(0).id();
    let reply = edit(&n, item, &[other_rev], "Burns", b"Steps", &[]).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    let reply = edit(&n, item, &[first.id()], "Burns", b"Steps", &[keep(other_rev, 1)]).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.text().contains("A file to keep"));
    let reply = edit(&n, item, &[], "Burns", b"Steps", &[]).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);

    assert_eq!(n.store.item_revisions(item).unwrap().len(), 1, "nothing was saved");
}

#[tokio::test]
async fn private_items_cannot_be_edited() {
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
            extra: Default::default(),
        },
        &author,
    )
    .unwrap();
    n.store.put_object(private.bytes()).unwrap();
    let item = private.id();
    for path in ["edit", "merge", "history"] {
        assert_eq!(
            get(&n.app, &format!("/item/{item}/{path}")).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(
        get(&n.app, &format!("/item/{item}/version/{item}")).await.status,
        StatusCode::NOT_FOUND
    );
    let reply = edit(&n, item, &[item], "Mine now", b"Steps", &[]).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(n.store.item_revisions(item).unwrap().len(), 1);
}
