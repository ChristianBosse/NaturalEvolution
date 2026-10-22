//! The first-run setup wizard (D-052).
//!
//! Until the node has an admin, every page leads here. The person names the node and creates the
//! admin account, then sees the node's key and address.
//!
//! Anyone on the network could open /setup before the owner does and take the node over. So the
//! wizard asks for a one-time setup code that `nevo` prints in its console at startup: only
//! someone who can see the console (the person who started the node) can finish setup.

use axum::extract::State;
use axum::http::header::{self, HeaderMap};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use maud::{html, Markup};
use nevo_format::{KeyKind, PublicKey};
use zeroize::Zeroizing;

use crate::accounts::{self, AccountError, Role, MIN_PASSPHRASE_CHARS};
use crate::app::{AppError, AppState};
use crate::auth::{self, csrf_field, form_page, Fields, Viewer, FORM_COOKIE};
use crate::pages;

const MAX_NODE_NAME_CHARS: usize = 100;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/setup", get(setup_form).post(setup))
        .layer(axum::extract::DefaultBodyLimit::max(auth::MAX_FORM_BYTES))
}

/// What the setup form shows again after a mistake. Passphrases are never sent back.
#[derive(Default)]
struct Typed {
    node_name: String,
    admin_name: String,
    error: Option<String>,
}

async fn setup_form(State(state): State<AppState>, viewer: Viewer) -> Response {
    if state.accounts.has_admin() {
        return Redirect::to("/").into_response();
    }
    let (token, new_cookie) = match viewer.anonymous_form_token() {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let mut response = form_page(StatusCode::OK, setup_page(&token, &Typed::default()));
    if let Some(cookie) = new_cookie {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

async fn setup(
    State(state): State<AppState>,
    viewer: Viewer,
    headers: HeaderMap,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if state.accounts.has_admin() {
        return Redirect::to("/").into_response();
    }
    let form = Fields(form);
    if let Err(e) = viewer.check_csrf(form.get("csrf")) {
        return e.into_response();
    }
    let token = viewer.csrf_token().unwrap_or_default();
    let mut typed = Typed {
        node_name: form.get("node_name").trim().to_string(),
        admin_name: form.get("admin_name").trim().to_string(),
        error: None,
    };
    let refuse = |status: StatusCode, typed: &mut Typed, message: &str| {
        typed.error = Some(message.to_string());
        form_page(status, setup_page(&token, typed))
    };
    // The code is checked first, so a stranger learns nothing else and costs no Argon2 work.
    if !state.accounts.check_setup_code(form.get("code")) {
        return refuse(
            StatusCode::FORBIDDEN,
            &mut typed,
            "That setup code is not right. It is shown in the console where nevo was started.",
        );
    }
    if typed.node_name.is_empty() || typed.node_name.chars().count() > MAX_NODE_NAME_CHARS {
        return refuse(
            StatusCode::BAD_REQUEST,
            &mut typed,
            &format!("Please name this NEVO (at most {MAX_NODE_NAME_CHARS} characters)."),
        );
    }
    let passphrase = Zeroizing::new(form.get("passphrase").to_string());
    if passphrase.as_str() != form.get("repeat") {
        return refuse(
            StatusCode::BAD_REQUEST,
            &mut typed,
            "The two passphrases are different.",
        );
    }
    if let Err(e) = accounts::check_passphrase(&passphrase) {
        return refuse(StatusCode::BAD_REQUEST, &mut typed, &e.to_string());
    }

    let (store, node, accounts) = (state.store.clone(), state.node.clone(), state.accounts.clone());
    let (node_name, admin_name) = (typed.node_name.clone(), typed.admin_name.clone());
    let result = tokio::task::spawn_blocking(move || {
        let (account, key) = accounts.create(&store, &node, &admin_name, &passphrase, Role::Admin, true)?;
        accounts.set_node_name(&node_name)?;
        // The node's own key record tells other nodes its name.
        accounts::publish_key_record(&store, &node, &node, KeyKind::Node, &node_name)?;
        accounts.start_session(account, &key)
    })
    .await;
    let session = match result {
        Ok(Ok(session)) => session,
        Ok(Err(AccountError::AlreadySetUp)) => return Redirect::to("/").into_response(),
        Ok(Err(e @ (AccountError::Invalid(_) | AccountError::NameTaken))) => {
            return refuse(StatusCode::BAD_REQUEST, &mut typed, &e.to_string())
        }
        Ok(Err(e)) => return AppError::from(e).into_response(),
        Err(e) => return AppError::Internal(e.to_string()).into_response(),
    };

    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("this-device:8080")
        .to_string();
    let page = done_page(&typed.node_name, &state.node.public(), &session.account.key, &host);
    let mut response = form_page(StatusCode::OK, page);
    let cookies = response.headers_mut();
    cookies.append(header::SET_COOKIE, auth::session_cookie(&session.cookie));
    cookies.append(header::SET_COOKIE, auth::clear_cookie(FORM_COOKIE));
    response
}

fn setup_page(token: &str, typed: &Typed) -> Markup {
    pages::layout(
        "Set up this NEVO",
        html! {
            h1 { "Set up this NEVO" }
            p { "Name this NEVO and create the admin account. The admin makes accounts for the people who will add guides. Reading never needs an account." }
            p.notice {
                "Anyone on this network can open this page. To prove you started this NEVO, type the "
                strong { "setup code" } " shown in the console (terminal window) where " code { "nevo" } " is running."
            }
            @if let Some(error) = &typed.error { p.error role="alert" { (error) } }
            form method="post" action="/setup" {
                (csrf_field(token))
                label for="code" { "Setup code " span.hint { "(from the console, like abcd-efgh-jkmn)" } }
                input type="text" id="code" name="code" required autocomplete="off" autocapitalize="none" spellcheck="false";
                label for="node_name" { "Name of this NEVO " span.hint { "(for example: Riverside School Library)" } }
                input type="text" id="node_name" name="node_name" required maxlength="100" value=(typed.node_name);
                label for="admin_name" { "Your name " span.hint { "(the admin account; others will see it next to what you add)" } }
                input type="text" id="admin_name" name="admin_name" required maxlength="64" autocomplete="username" value=(typed.admin_name);
                label for="passphrase" { "Passphrase " span.hint { "(at least " (MIN_PASSPHRASE_CHARS) " characters; a few words are easy to remember)" } }
                input type="password" id="passphrase" name="passphrase" required autocomplete="new-password";
                label for="repeat" { "Passphrase again" }
                input type="password" id="repeat" name="repeat" required autocomplete="new-password";
                p.muted { "Your passphrase also protects your signing key. If it is forgotten, nobody can recover that key." }
                button type="submit" { "Set up" }
            }
        },
    )
}

fn done_page(node_name: &str, node_key: &PublicKey, admin_key: &PublicKey, host: &str) -> Markup {
    let address = format!("http://{host}/");
    pages::layout(
        "Ready",
        html! {
            h1 { (node_name) " is ready" }
            p { "You are signed in as its admin." }
            h2 { "Address" }
            p { "People on this network open " strong { a href="/" { (address) } } " in a browser." }
            h2 { "Keys" }
            p.muted { "Other NEVOs know this node and you by these keys. They are public; it is safe to share them." }
            p { "This NEVO: " code { (node_key) } }
            p { "You: " code { (admin_key) } }
            h2 { "Next" }
            ul {
                li { a href="/accounts" { "Make accounts" } " for the people who will add guides." }
                li { a href="/account/key" { "Copy your key to your phone" } " with a QR code." }
                li { a href="/add" { "Add the first guide." } }
            }
        },
    )
}
