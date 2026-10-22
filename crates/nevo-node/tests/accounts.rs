//! Accounts, sign-in, roles, forged forms and the setup wizard, through the real router.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use nevo_format::{Id, KeyKind, Keypair, PublicKey, Visibility};
use nevo_node::accounts::{self, Accounts, Role, ACCOUNTS_FILE};
use nevo_node::{router, AppState};
use nevo_store::Store;
use tower::ServiceExt;

const HOST: &str = "nevo.test";
const ORIGIN: &str = "http://nevo.test";
const PASSPHRASE: &str = "river stone lantern";

struct Node {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    key: Arc<Keypair>,
    accounts: Arc<Accounts>,
    app: Router,
}

/// A new node that has not been set up.
fn node() -> Node {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().join("library")).unwrap());
    let key = Arc::new(Keypair::from_secret([7; 32]));
    let accounts = Arc::new(Accounts::open(dir.path().join(ACCOUNTS_FILE)).unwrap());
    let app = router(AppState::new(store.clone(), key.clone(), accounts.clone()));
    Node {
        _dir: dir,
        store,
        key,
        accounts,
        app,
    }
}

/// A node with an admin ("Admin") and a contributor ("Writer").
fn set_up_node() -> Node {
    let n = node();
    n.accounts
        .create(&n.store, &n.key, "Admin", PASSPHRASE, Role::Admin, true)
        .unwrap();
    n.accounts
        .create(&n.store, &n.key, "Writer", PASSPHRASE, Role::Contributor, false)
        .unwrap();
    n
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Reply {
    fn header(&self, name: header::HeaderName) -> String {
        self.headers
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }

    /// The hidden form token on this page.
    fn csrf(&self) -> String {
        self.body
            .split("name=\"csrf\" value=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("no form token on the page: {}", self.body))
            .to_string()
    }
}

/// A browser: keeps cookies, and sends `Origin` like a browser posting a form of this node.
#[derive(Default, Clone)]
struct Browser {
    cookies: HashMap<String, String>,
}

impl Browser {
    async fn send(&mut self, app: &Router, mut request: Request<Body>) -> Reply {
        let cookies: Vec<String> = self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect();
        if !cookies.is_empty() {
            request
                .headers_mut()
                .insert(header::COOKIE, cookies.join("; ").parse().unwrap());
        }
        request.headers_mut().insert(header::HOST, HOST.parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        for set in headers.get_all(header::SET_COOKIE) {
            let set = set.to_str().unwrap();
            let (name, rest) = set.split_once('=').unwrap();
            let value = rest.split(';').next().unwrap();
            if set.contains("Max-Age=0") {
                self.cookies.remove(name);
            } else {
                self.cookies.insert(name.to_string(), value.to_string());
            }
        }
        let body = response.into_body().collect().await.unwrap().to_bytes();
        Reply {
            status,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        }
    }

    async fn get(&mut self, app: &Router, uri: &str) -> Reply {
        self.send(app, Request::get(uri).body(Body::empty()).unwrap()).await
    }

    async fn post(&mut self, app: &Router, uri: &str, fields: &[(&str, &str)]) -> Reply {
        self.post_from(app, uri, fields, Some(ORIGIN)).await
    }

    async fn post_from(&mut self, app: &Router, uri: &str, fields: &[(&str, &str)], origin: Option<&str>) -> Reply {
        let body: Vec<String> = fields
            .iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect();
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        self.send(app, request.body(Body::from(body.join("&"))).unwrap()).await
    }

    /// Posts the add form (multipart). `csrf: None` leaves the token out.
    async fn add(&mut self, app: &Router, csrf: Option<&str>, title: &str, extra: &[(&str, &str)]) -> Reply {
        let boundary = "nevo-test-boundary-91c2";
        let mut fields: Vec<(&str, &str)> = Vec::new();
        if let Some(csrf) = csrf {
            fields.push(("csrf", csrf));
        }
        fields.extend([("title", title), ("topics", "general"), ("text", "Some steps.")]);
        fields.extend_from_slice(extra);
        let mut body = String::new();
        for (name, value) in fields {
            body.push_str(&format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            ));
        }
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\nattached\r\n--{boundary}--\r\n"
        ));
        let request = Request::post("/add")
            .header(header::ORIGIN, ORIGIN)
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        self.send(app, request).await
    }

    async fn sign_in(&mut self, app: &Router, name: &str, passphrase: &str) -> Reply {
        let form = self.get(app, "/signin").await;
        self.post(
            app,
            "/signin",
            &[("csrf", &form.csrf()), ("name", name), ("passphrase", passphrase)],
        )
        .await
    }

    /// Signs in and returns the add form's token.
    async fn sign_in_to_add(&mut self, app: &Router, name: &str) -> String {
        assert_eq!(self.sign_in(app, name, PASSPHRASE).await.status, StatusCode::SEE_OTHER);
        self.get(app, "/add").await.csrf()
    }
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn item_of(reply: &Reply) -> Id {
    reply
        .header(header::LOCATION)
        .trim_start_matches("/item/")
        .parse()
        .unwrap()
}

fn account_key(n: &Node, name: &str) -> PublicKey {
    n.accounts
        .list()
        .unwrap()
        .into_iter()
        .find(|a| a.name == name)
        .unwrap()
        .key
}

// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn setup_needs_the_code_and_happens_once() {
    let n = node();
    let mut owner = Browser::default();

    // Every page leads to the wizard until there is an admin.
    for page in ["/", "/topics", "/add", "/signin", "/account"] {
        let reply = owner.get(&n.app, page).await;
        assert_eq!(reply.status, StatusCode::SEE_OTHER, "{page}");
        assert_eq!(reply.header(header::LOCATION), "/setup", "{page}");
    }
    assert_eq!(owner.get(&n.app, "/health").await.status, StatusCode::OK);

    let form = owner.get(&n.app, "/setup").await;
    assert_eq!(form.status, StatusCode::OK);
    assert!(form.body.contains("setup code"));
    let csrf = form.csrf();
    let code = n.accounts.setup_code().unwrap();
    let fields = |csrf: &'static str, code: &'static str| {
        [
            ("csrf", csrf),
            ("node_name", "Riverside Library"),
            ("admin_name", "Amina"),
            ("passphrase", PASSPHRASE),
            ("repeat", PASSPHRASE),
            ("code", code),
        ]
    };
    let csrf: &'static str = csrf.leak();

    // A wrong code is refused before anything happens.
    let reply = owner.post(&n.app, "/setup", &fields(csrf, "abcd-efgh-jkmn")).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(reply.body.contains("setup code is not right"));
    assert!(!n.accounts.has_admin());

    // So is a form without the right form token.
    let right: &'static str = code.clone().leak();
    let reply = owner.post(&n.app, "/setup", &fields("wrong", right)).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!n.accounts.has_admin());

    // The code may be typed in capitals and without dashes.
    let typed: &'static str = code.to_uppercase().replace('-', "").leak();
    let done = owner.post(&n.app, "/setup", &fields(csrf, typed)).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert!(done.body.contains("Riverside Library is ready"));
    assert!(done.body.contains(&n.key.public().to_string()), "shows the node key");
    assert!(done.body.contains("http://nevo.test/"), "shows the address");
    let admin = account_key(&n, "Amina");
    assert!(done.body.contains(&admin.to_string()), "shows the admin's key");
    assert!(n.accounts.has_admin());
    assert_eq!(n.accounts.setup_code(), None);
    assert_eq!(n.accounts.node_name().unwrap().as_deref(), Some("Riverside Library"));

    // Both key records were published: the admin's (a person) and the node's, with its name.
    let node_records = n.store.key_records(n.key.public()).unwrap();
    assert_eq!(node_records.len(), 1);
    assert_eq!(node_records[0].object.kind, KeyKind::Node);
    assert_eq!(node_records[0].object.name, "Riverside Library");
    assert_eq!(n.store.key_records(admin).unwrap()[0].object.kind, KeyKind::Person);

    // The owner is signed in as admin, and the wizard is gone.
    let account = owner.get(&n.app, "/account").await;
    assert_eq!(account.status, StatusCode::OK);
    assert!(account.body.contains("Amina"));
    assert_eq!(owner.get(&n.app, "/").await.status, StatusCode::OK);
    let mut stranger = Browser::default();
    assert_eq!(stranger.get(&n.app, "/setup").await.header(header::LOCATION), "/");
    let reply = stranger.post(&n.app, "/setup", &fields("x", typed)).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert_eq!(n.accounts.list().unwrap().len(), 1);
}

#[tokio::test]
async fn sign_in_and_out() {
    let n = set_up_node();
    let mut browser = Browser::default();
    assert_eq!(
        browser.get(&n.app, "/account").await.header(header::LOCATION),
        "/signin?next=/account"
    );

    let form = browser.get(&n.app, "/signin?next=/account").await;
    assert!(form.body.contains("value=\"/account\""));
    let reply = browser
        .post(
            &n.app,
            "/signin",
            &[
                ("csrf", &form.csrf()),
                ("name", "writer"),
                ("passphrase", PASSPHRASE),
                ("next", "/account"),
            ],
        )
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert_eq!(reply.header(header::LOCATION), "/account");
    let cookie = reply.header(header::SET_COOKIE);
    assert!(cookie.starts_with("nevo_session="));
    for attribute in ["HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(cookie.contains(attribute), "{cookie}");
    }

    let account = browser.get(&n.app, "/account").await;
    assert_eq!(account.status, StatusCode::OK);
    assert!(account.body.contains("Writer"));
    assert!(account.body.contains("Contributor"));
    assert_eq!(account.header(header::CACHE_CONTROL), "no-store");

    // An outside address after sign-in is ignored.
    let mut other = Browser::default();
    let form = other.get(&n.app, "/signin").await;
    let reply = other
        .post(
            &n.app,
            "/signin",
            &[
                ("csrf", &form.csrf()),
                ("name", "Writer"),
                ("passphrase", PASSPHRASE),
                ("next", "//evil.example/"),
            ],
        )
        .await;
    assert_eq!(reply.header(header::LOCATION), "/");

    let copied = browser.clone();
    let reply = browser.post(&n.app, "/signout", &[("csrf", &account.csrf())]).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert!(!browser.cookies.contains_key("nevo_session"));
    assert_eq!(browser.get(&n.app, "/account").await.status, StatusCode::SEE_OTHER);
    // The session ended on the node too: a copy of the old cookie no longer works.
    let mut copied = copied;
    let reply = copied.get(&n.app, "/account").await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert!(
        reply.header(header::SET_COOKIE).contains("Max-Age=0"),
        "the dead cookie is cleared"
    );
}

#[tokio::test]
async fn a_wrong_passphrase_is_refused() {
    let n = set_up_node();
    let mut browser = Browser::default();
    let reply = browser.sign_in(&n.app, "Writer", "not the passphrase").await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(reply.body.contains("Wrong name or passphrase."));
    assert!(reply.body.contains("value=\"Writer\""), "the name is kept");
    assert!(!browser.cookies.contains_key("nevo_session"));

    // An unknown name gets the same answer.
    let reply = browser.sign_in(&n.app, "Nobody", PASSPHRASE).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(reply.body.contains("Wrong name or passphrase."));

    // A turned-off account can't sign in.
    let writer = n
        .accounts
        .list()
        .unwrap()
        .into_iter()
        .find(|a| a.name == "Writer")
        .unwrap();
    n.accounts.set_disabled(writer.id, true).unwrap();
    let reply = browser.sign_in(&n.app, "Writer", PASSPHRASE).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(!browser.cookies.contains_key("nevo_session"));
}

#[tokio::test]
async fn forged_forms_are_refused() {
    let n = set_up_node();
    let mut browser = Browser::default();
    let csrf = browser.sign_in_to_add(&n.app, "Writer").await;

    // No token, or another session's token.
    let reply = browser.add(&n.app, None, "No token", &[]).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    let reply = browser.add(&n.app, Some(&"0".repeat(64)), "Wrong token", &[]).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(n.store.recent_tips(10).unwrap().is_empty());

    // Sent from another site, or with the origin hidden, even with the right token.
    let fields = [("csrf", csrf.as_str())];
    for origin in [
        Some("http://evil.example"),
        Some("null"),
        Some("http://nevo.test.evil.example"),
        None,
    ] {
        let reply = browser.post_from(&n.app, "/signout", &fields, origin).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{origin:?}");
    }
    let reply = browser
        .post_from(
            &n.app,
            "/signin",
            &[("name", "Writer"), ("passphrase", PASSPHRASE)],
            Some("http://evil.example"),
        )
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(
        browser.get(&n.app, "/account").await.status,
        StatusCode::OK,
        "still signed in"
    );

    // Old browsers send no Origin, but a Referer from this node is enough.
    let request = Request::post("/signout")
        .header(header::REFERER, "http://nevo.test/account")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("csrf={csrf}")))
        .unwrap();
    assert_eq!(browser.send(&n.app, request).await.status, StatusCode::SEE_OTHER);
    assert_eq!(
        browser.get(&n.app, "/account").await.status,
        StatusCode::SEE_OTHER,
        "signed out"
    );

    // Pages ask browsers to send the origin on form posts.
    assert_eq!(
        browser.get(&n.app, "/").await.header(header::REFERRER_POLICY),
        "same-origin"
    );
}

#[tokio::test]
async fn readers_browse_but_cannot_add() {
    let n = set_up_node();
    let mut writer = Browser::default();
    let csrf = writer.sign_in_to_add(&n.app, "Writer").await;
    let added = writer.add(&n.app, Some(&csrf), "Clean water", &[]).await;
    assert_eq!(added.status, StatusCode::SEE_OTHER);

    let mut reader = Browser::default();
    assert!(reader.get(&n.app, "/").await.body.contains("Clean water"));
    assert_eq!(reader.get(&n.app, "/topics").await.status, StatusCode::OK);
    let page = reader.get(&n.app, &added.header(header::LOCATION)).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.header(header::CACHE_CONTROL).is_empty(),
        "public pages stay cacheable"
    );

    let form = reader.get(&n.app, "/add").await;
    assert_eq!(form.status, StatusCode::SEE_OTHER);
    assert_eq!(form.header(header::LOCATION), "/signin?next=/add");
    let reply = reader.add(&n.app, Some(&csrf), "Sneaky", &[]).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(n.store.recent_tips(10).unwrap().len(), 1);

    // Contributors can't manage accounts; readers are sent to sign in.
    assert_eq!(writer.get(&n.app, "/accounts").await.status, StatusCode::FORBIDDEN);
    assert_eq!(reader.get(&n.app, "/accounts").await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn admins_manage_accounts() {
    let n = set_up_node();
    let mut admin = Browser::default();
    assert_eq!(
        admin.sign_in(&n.app, "Admin", PASSPHRASE).await.status,
        StatusCode::SEE_OTHER
    );
    let page = admin.get(&n.app, "/accounts").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Writer"));
    let csrf = page.csrf();

    let fields = [
        ("csrf", csrf.as_str()),
        ("name", "Bao"),
        ("role", "contributor"),
        ("passphrase", "short"),
    ];
    let reply = admin.post(&n.app, "/accounts", &fields).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.body.contains("at least 8 characters"));
    let fields = [
        ("csrf", csrf.as_str()),
        ("name", "Bao"),
        ("role", "contributor"),
        ("passphrase", PASSPHRASE),
    ];
    assert_eq!(
        admin.post(&n.app, "/accounts", &fields).await.status,
        StatusCode::SEE_OTHER
    );
    assert!(
        n.store.key_records(account_key(&n, "Bao")).unwrap().len() == 1,
        "key record published"
    );

    let mut bao = Browser::default();
    assert_eq!(
        bao.sign_in(&n.app, "Bao", PASSPHRASE).await.status,
        StatusCode::SEE_OTHER
    );
    let bao_id = n
        .accounts
        .list()
        .unwrap()
        .into_iter()
        .find(|a| a.name == "Bao")
        .unwrap()
        .id;
    let reply = admin
        .post(
            &n.app,
            &format!("/accounts/{bao_id}/active"),
            &[("csrf", &csrf), ("active", "0")],
        )
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert_eq!(
        bao.get(&n.app, "/account").await.status,
        StatusCode::SEE_OTHER,
        "signed out"
    );

    // The last admin can't step down.
    let admin_id = n
        .accounts
        .list()
        .unwrap()
        .into_iter()
        .find(|a| a.name == "Admin")
        .unwrap()
        .id;
    let reply = admin
        .post(
            &n.app,
            &format!("/accounts/{admin_id}/role"),
            &[("csrf", &csrf), ("role", "contributor")],
        )
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(n.accounts.get(admin_id).unwrap().unwrap().role, Role::Admin);
}

#[tokio::test]
async fn private_items_are_seen_only_by_their_author() {
    let n = set_up_node();
    n.accounts
        .create(&n.store, &n.key, "Other", PASSPHRASE, Role::Contributor, false)
        .unwrap();
    let mut author = Browser::default();
    let csrf = author.sign_in_to_add(&n.app, "Writer").await;
    let logged = n.store.log_position(n.key.public()).unwrap();
    let added = author
        .add(&n.app, Some(&csrf), "My medical notes", &[("private", "1")])
        .await;
    assert_eq!(added.status, StatusCode::SEE_OTHER, "{}", added.body);
    let url = added.header(header::LOCATION);
    let item = item_of(&added);
    let rev = n.store.item_tips(item).unwrap()[0].clone();
    assert_eq!(rev.object.vis, Visibility::Private);
    assert_eq!(n.store.log_position(n.key.public()).unwrap(), logged, "never published");
    let file = format!("/file/{}/0/guide.md", rev.id());

    let page = author.get(&n.app, &url).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Private: only you can see it"));
    assert!(author.get(&n.app, "/").await.body.contains("My medical notes"));
    assert!(author
        .get(&n.app, "/topic/general")
        .await
        .body
        .contains("My medical notes"));
    let served = author.get(&n.app, &file).await;
    assert_eq!(served.status, StatusCode::OK);
    assert_eq!(served.header(header::CACHE_CONTROL), "no-store");

    let mut other = Browser::default();
    other.sign_in_to_add(&n.app, "Other").await;
    let mut reader = Browser::default();
    for viewer in [&mut other, &mut reader] {
        assert_eq!(viewer.get(&n.app, &url).await.status, StatusCode::NOT_FOUND);
        assert_eq!(viewer.get(&n.app, &file).await.status, StatusCode::NOT_FOUND);
        assert!(!viewer.get(&n.app, "/").await.body.contains("My medical notes"));
        assert!(!viewer
            .get(&n.app, "/topic/general")
            .await
            .body
            .contains("My medical notes"));
    }

    // Signed out, the author is a reader too.
    let account = author.get(&n.app, "/account").await;
    author.post(&n.app, "/signout", &[("csrf", &account.csrf())]).await;
    assert_eq!(author.get(&n.app, &url).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn revisions_are_signed_by_the_contributor() {
    let n = set_up_node();
    let mut writer = Browser::default();
    let csrf = writer.sign_in_to_add(&n.app, "Writer").await;
    let added = writer.add(&n.app, Some(&csrf), "Boiling water", &[]).await;
    assert_eq!(added.status, StatusCode::SEE_OTHER);

    let rev = n.store.item_tips(item_of(&added)).unwrap()[0].clone();
    let writer_key = account_key(&n, "Writer");
    assert_eq!(rev.object.author, writer_key);
    assert_ne!(rev.object.author, n.key.public());
    // The store checked the signature against the author on the way in; check it again.
    let bytes = n.store.object_bytes(rev.id()).unwrap().unwrap();
    assert_eq!(
        nevo_format::open::<nevo_format::Revision>(&bytes)
            .unwrap()
            .object
            .author,
        writer_key
    );

    // The node's log carries it, and the node key signs the log head.
    let (entries, last) = (n.store.log_entries(n.key.public(), 0, 10).unwrap(), rev.id());
    assert_eq!(entries.last().unwrap().1, last);
    let head = n.store.sign_head(&n.key).unwrap().unwrap();
    assert_eq!(head.object.node, n.key.public());

    let page = writer.get(&n.app, &added.header(header::LOCATION)).await;
    assert!(page.body.contains("Added on this NEVO"));
}

#[tokio::test]
async fn key_export_needs_the_passphrase() {
    let n = set_up_node();
    let mut writer = Browser::default();
    let mut reader = Browser::default();
    assert_eq!(
        reader.get(&n.app, "/account/key").await.header(header::LOCATION),
        "/signin?next=/account/key"
    );

    writer.sign_in_to_add(&n.app, "Writer").await;
    let form = writer.get(&n.app, "/account/key").await;
    assert_eq!(form.status, StatusCode::OK);
    assert!(!form.body.contains("NEVO-SECRET"));
    let csrf = form.csrf();

    for wrong in ["", "not the passphrase"] {
        let reply = writer
            .post(&n.app, "/account/key", &[("csrf", &csrf), ("passphrase", wrong)])
            .await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert!(!reply.body.contains("NEVO-SECRET"));
        assert!(!reply.body.contains("<svg"));
    }
    let reply = writer.post(&n.app, "/account/key", &[("passphrase", PASSPHRASE)]).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "the form token is still needed");

    let reply = writer
        .post(&n.app, "/account/key", &[("csrf", &csrf), ("passphrase", PASSPHRASE)])
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.header(header::CACHE_CONTROL), "no-store");
    assert!(reply.body.contains("<svg"), "an inline QR code");
    assert!(!reply.body.contains("<?xml"));
    assert!(!reply.body.contains("<script"));
    let text = reply
        .body
        .split("NEVO-SECRET-1:")
        .nth(1)
        .map(|rest| format!("NEVO-SECRET-1:{}", rest.split('<').next().unwrap()))
        .unwrap();
    let key = accounts::parse_secret_text(&text).unwrap();
    assert_eq!(key.public(), account_key(&n, "Writer"));
}
