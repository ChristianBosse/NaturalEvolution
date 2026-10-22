//! Who is asking: sessions, roles, sign-in and protection against forged forms.
//!
//! - **Readers** need no account (D-008). **Contributors** may add. **Admins** also manage
//!   accounts. Every request passes through [`middleware`], which finds the session and stores a
//!   [`Viewer`] for the handlers.
//! - **Sessions** use a random cookie: `HttpOnly` (scripts can't read it) and `SameSite=Strict`
//!   (other sites can't send it). It can't be `Secure`: a NEVO is reached over plain HTTP on a
//!   local network with no certificate, and browsers drop `Secure` cookies there. So anyone who
//!   can watch the Wi-Fi traffic can copy a session cookie (D-051).
//! - **Forged forms** (CSRF): every POST must come from a page of this node, by its `Origin`
//!   header (or `Referer` for old browsers), and carry the session's form token in a hidden
//!   `csrf` field. Forms used before signing in (sign-in, setup) use a random cookie instead,
//!   compared with the hidden field ("double submit").

use std::sync::Arc;

use axum::extract::{FromRequestParts, Query, Request, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use maud::{html, Markup};
use nevo_format::{Revision, Visibility};

use crate::accounts::{self, Account, AccountError, Role, Session, SESSION_SECONDS};
use crate::app::{blocking, AppError, AppResult, AppState};
use crate::pages;

pub const SESSION_COOKIE: &str = "nevo_session";
/// Form token for people who are not signed in yet.
pub const FORM_COOKIE: &str = "nevo_form";
/// Form posts handled here are small; the 4 GB upload limit is only for /add.
pub const MAX_FORM_BYTES: usize = 64 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/signin", get(sign_in_form).post(sign_in))
        .route("/signout", post(sign_out))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_FORM_BYTES))
}

impl From<AccountError> for AppError {
    fn from(e: AccountError) -> AppError {
        match e {
            AccountError::Internal(m) => AppError::Internal(m),
            other => AppError::BadRequest(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The viewer

/// The person making a request: signed in or an anonymous reader.
#[derive(Clone, Default)]
pub struct Viewer {
    session: Option<Arc<Session>>,
    form_cookie: Option<String>,
}

impl Viewer {
    pub fn account(&self) -> Option<&Account> {
        self.session.as_ref().map(|s| &s.account)
    }

    pub fn session(&self) -> Option<&Arc<Session>> {
        self.session.as_ref()
    }

    /// Private revisions are shown only to their author (D-009).
    pub fn can_see(&self, revision: &Revision) -> bool {
        revision.vis != Visibility::Private || self.account().is_some_and(|a| a.key == revision.author)
    }

    /// The token forms must carry: the session's, or the anonymous form cookie.
    pub fn csrf_token(&self) -> Option<String> {
        match &self.session {
            Some(session) => Some(session.csrf_token()),
            None => self.form_cookie.clone(),
        }
    }

    /// Checks the `csrf` field of a submitted form.
    pub fn check_csrf(&self, sent: &str) -> AppResult<()> {
        match self.csrf_token() {
            Some(expected) if accounts::same_bytes(expected.as_bytes(), sent.as_bytes()) => Ok(()),
            _ => Err(expired_form()),
        }
    }

    /// The token for a form shown before sign-in, and the cookie to set when it is new.
    pub fn anonymous_form_token(&self) -> AppResult<(String, Option<HeaderValue>)> {
        if let Some(token) = self.csrf_token() {
            return Ok((token, None));
        }
        let token = accounts::random_token()?;
        let cookie = cookie_header(&format!("{FORM_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict"));
        Ok((token, Some(cookie)))
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Viewer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Viewer, Self::Rejection> {
        Ok(parts.extensions.get::<Viewer>().cloned().unwrap_or_default())
    }
}

/// A signed-in person: a contributor or an admin. Readers are sent to sign in.
pub struct Contributor(pub Arc<Session>);

impl<S: Send + Sync> FromRequestParts<S> for Contributor {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Contributor, Response> {
        match parts.extensions.get::<Viewer>().and_then(|v| v.session.clone()) {
            Some(session) => Ok(Contributor(session)),
            None => Err(sign_in_needed(parts)),
        }
    }
}

/// A signed-in admin.
pub struct Admin(pub Arc<Session>);

impl<S: Send + Sync> FromRequestParts<S> for Admin {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Admin, Response> {
        match parts.extensions.get::<Viewer>().and_then(|v| v.session.clone()) {
            Some(session) if session.account.role == Role::Admin => Ok(Admin(session)),
            Some(_) => Err(AppError::Forbidden("Only an admin of this NEVO can do this.".into()).into_response()),
            None => Err(sign_in_needed(parts)),
        }
    }
}

fn sign_in_needed(parts: &Parts) -> Response {
    if parts.method == Method::GET || parts.method == Method::HEAD {
        Redirect::to(&format!("/signin?next={}", encode_query(parts.uri.path()))).into_response()
    } else {
        AppError::Forbidden("Please sign in first.".into()).into_response()
    }
}

// ---------------------------------------------------------------------------------------------
// Middleware

/// Runs before every handler: refuses cross-site posts, finds the session, and sends everyone to
/// the setup wizard until the node has an admin.
pub async fn middleware(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let posting = !matches!(*request.method(), Method::GET | Method::HEAD);
    if posting && !same_origin(request.headers(), request.uri()) {
        return AppError::Forbidden("This form came from another site or page, so it was refused.".into())
            .into_response();
    }

    let session_cookie = cookie(request.headers(), SESSION_COOKIE);
    let session = match session_cookie.clone() {
        Some(value) => {
            let accounts = state.accounts.clone();
            match blocking(move || Ok(accounts.session(&value)?)).await {
                Ok(session) => session,
                Err(e) => return e.into_response(),
            }
        }
        None => None,
    };
    let path = request.uri().path();
    if !state.accounts.has_admin() && path != "/setup" && path != "/health" {
        return Redirect::to("/setup").into_response();
    }

    let stale_cookie = session_cookie.is_some() && session.is_none();
    let signed_in = session.is_some();
    let form_cookie = cookie(request.headers(), FORM_COOKIE).filter(|t| t.len() == 64);
    request.extensions_mut().insert(Viewer {
        session: session.map(Arc::new),
        form_cookie,
    });
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if stale_cookie {
        headers.append(header::SET_COOKIE, clear_cookie(SESSION_COOKIE));
    }
    // Pages for a signed-in person may list their private items: keep them out of caches.
    if signed_in && !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// Whether a POST came from a page of this node: its `Origin` (or, if a browser sent none, its
/// `Referer`) must name the host the request was sent to. This needs the `same-origin` referrer
/// policy: with `no-referrer`, browsers send `Origin: null` on every form post.
pub fn same_origin(headers: &HeaderMap, uri: &Uri) -> bool {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| uri.authority().map(|a| a.as_str()));
    let Some(host) = host else {
        return false;
    };
    let names_host = |url: &str| {
        let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://"));
        rest.and_then(|r| r.split(['/', '?', '#']).next())
            .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
    };
    if let Some(origin) = headers.get(header::ORIGIN) {
        return origin.to_str().is_ok_and(names_host);
    }
    headers
        .get(header::REFERER)
        .and_then(|r| r.to_str().ok())
        .is_some_and(names_host)
}

/// The first cookie called `name`.
fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(n, _)| *n == name)
        .map(|(_, value)| value.to_string())
}

fn cookie_header(text: &str) -> HeaderValue {
    HeaderValue::from_str(text).unwrap_or_else(|_| HeaderValue::from_static(""))
}

pub fn session_cookie(value: &str) -> HeaderValue {
    cookie_header(&format!(
        "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_SECONDS}"
    ))
}

pub fn clear_cookie(name: &str) -> HeaderValue {
    cookie_header(&format!("{name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"))
}

/// Percent-encodes a path for a query string value.
fn encode_query(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Where to go after signing in: a path on this node, never another site.
fn safe_next(next: &str) -> &str {
    if next.starts_with('/') && !next.starts_with("//") && !next.starts_with("/\\") {
        next
    } else {
        "/"
    }
}

// ---------------------------------------------------------------------------------------------
// Form helpers

/// Checks the `csrf` field of a form posted by a signed-in person.
pub fn check_session_csrf(session: &Session, sent: &str) -> AppResult<()> {
    if accounts::same_bytes(session.csrf_token().as_bytes(), sent.as_bytes()) {
        Ok(())
    } else {
        Err(expired_form())
    }
}

fn expired_form() -> AppError {
    AppError::Forbidden("This form has expired. Go back, reload the page and try again.".into())
}

/// The fields of a posted form, without needing a type for each form.
pub struct Fields(pub Vec<(String, String)>);

impl Fields {
    pub fn get(&self, name: &str) -> &str {
        self.0
            .iter()
            .find(|(n, _)| n == name)
            .map_or("", |(_, value)| value.as_str())
    }
}

/// A form handler's answer when a form is refused but can be shown again.
pub fn form_page(status: StatusCode, page: Markup) -> Response {
    (status, Html(page.into_string())).into_response()
}

pub fn csrf_field(token: &str) -> Markup {
    html! { input type="hidden" name="csrf" value=(token); }
}

// ---------------------------------------------------------------------------------------------
// Sign in and out

async fn sign_in_form(viewer: Viewer, Query(query): Query<Vec<(String, String)>>) -> AppResult<Response> {
    let next = query
        .iter()
        .find(|(n, _)| n == "next")
        .map_or("/", |(_, v)| safe_next(v));
    let (token, new_cookie) = viewer.anonymous_form_token()?;
    let mut response = form_page(StatusCode::OK, sign_in_page(&viewer, &token, next, "", None));
    if let Some(cookie) = new_cookie {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    Ok(response)
}

async fn sign_in(
    State(state): State<AppState>,
    viewer: Viewer,
    headers: HeaderMap,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    let form = Fields(form);
    if let Err(e) = viewer.check_csrf(form.get("csrf")) {
        return e.into_response();
    }
    let next = safe_next(form.get("next")).to_string();
    let name = form.get("name").to_string();
    let passphrase = zeroize::Zeroizing::new(form.get("passphrase").to_string());
    let accounts = state.accounts.clone();
    let old_cookie = cookie(&headers, SESSION_COOKIE);
    let typed = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let session = accounts.sign_in(&typed, &passphrase)?;
        // Someone else may have been signed in on this browser: end their session.
        if let Some(old) = old_cookie {
            accounts.sign_out(&old)?;
        }
        Ok::<_, AccountError>(session)
    })
    .await;
    match result {
        Ok(Ok(session)) => {
            let mut response = Redirect::to(&next).into_response();
            response
                .headers_mut()
                .append(header::SET_COOKIE, session_cookie(&session.cookie));
            response
        }
        Ok(Err(e @ (AccountError::WrongPassphrase | AccountError::Disabled))) => {
            let token = viewer.csrf_token().unwrap_or_default();
            form_page(
                StatusCode::UNAUTHORIZED,
                sign_in_page(&viewer, &token, &next, &name, Some(&e.to_string())),
            )
        }
        Ok(Err(e)) => AppError::from(e).into_response(),
        Err(e) => AppError::Internal(e.to_string()).into_response(),
    }
}

async fn sign_out(
    State(state): State<AppState>,
    viewer: Viewer,
    headers: HeaderMap,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if let Err(e) = viewer.check_csrf(Fields(form).get("csrf")) {
        return e.into_response();
    }
    if let Some(value) = cookie(&headers, SESSION_COOKIE) {
        let accounts = state.accounts.clone();
        if let Err(e) = blocking(move || Ok(accounts.sign_out(&value)?)).await {
            return e.into_response();
        }
    }
    let mut response = Redirect::to("/").into_response();
    response
        .headers_mut()
        .append(header::SET_COOKIE, clear_cookie(SESSION_COOKIE));
    response
}

fn sign_in_page(viewer: &Viewer, token: &str, next: &str, name: &str, error: Option<&str>) -> Markup {
    pages::layout(
        "Sign in",
        html! {
            h1 { "Sign in" }
            @if let Some(account) = viewer.account() {
                p.notice { "You are signed in as " (account.name) ". Signing in again replaces that." }
            }
            p.muted { "Reading needs no account. Sign in to add guides." }
            @if let Some(error) = error { p.error role="alert" { (error) } }
            form method="post" action="/signin" {
                (csrf_field(token))
                input type="hidden" name="next" value=(next);
                label for="name" { "Name" }
                input type="text" id="name" name="name" required maxlength="64" autocomplete="username" value=(name);
                label for="passphrase" { "Passphrase" }
                input type="password" id="passphrase" name="passphrase" required autocomplete="current-password";
                button type="submit" { "Sign in" }
            }
            p.muted { "No account? Ask an admin of this NEVO to make one for you." }
        },
    )
}
