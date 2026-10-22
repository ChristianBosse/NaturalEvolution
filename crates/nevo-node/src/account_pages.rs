//! Account pages: your own account, moving your key to another device, and (for admins) the
//! accounts of this node.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use maud::{html, Markup, PreEscaped};
use qrcode::render::svg;
use qrcode::{EcLevel, QrCode};
use zeroize::Zeroizing;

use crate::accounts::{self, Account, AccountError, Role, MIN_PASSPHRASE_CHARS};
use crate::app::{blocking, AppError, AppResult, AppState};
use crate::auth::{self, check_session_csrf, csrf_field, form_page, Admin, Contributor, Fields};
use crate::pages;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/account", get(account))
        .route("/account/passphrase", post(change_passphrase))
        .route("/account/key", get(key_form).post(key_export))
        .route("/accounts", get(list).post(create))
        .route("/accounts/{id}/role", post(set_role))
        .route("/accounts/{id}/active", post(set_active))
        .layer(axum::extract::DefaultBodyLimit::max(auth::MAX_FORM_BYTES))
}

// ---------------------------------------------------------------------------------------------
// Your account

async fn account(Contributor(session): Contributor) -> Response {
    form_page(
        StatusCode::OK,
        account_page(&session.account, &session.csrf_token(), None),
    )
}

async fn change_passphrase(
    State(state): State<AppState>,
    Contributor(session): Contributor,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    let form = Fields(form);
    let token = session.csrf_token();
    if let Err(e) = check_session_csrf(&session, form.get("csrf")) {
        return e.into_response();
    }
    let (old, new) = (
        Zeroizing::new(form.get("old").to_string()),
        Zeroizing::new(form.get("new").to_string()),
    );
    let message = if new.as_str() != form.get("repeat") {
        Err("The two new passphrases are different.".to_string())
    } else {
        let (accounts, id) = (state.accounts.clone(), session.account.id);
        match tokio::task::spawn_blocking(move || accounts.change_passphrase(id, &old, &new)).await {
            Ok(Ok(())) => Ok("Your passphrase is changed."),
            Ok(Err(e @ (AccountError::WrongPassphrase | AccountError::Invalid(_)))) => Err(e.to_string()),
            Ok(Err(e)) => return AppError::from(e).into_response(),
            Err(e) => return AppError::Internal(e.to_string()).into_response(),
        }
    };
    let status = if message.is_ok() {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    form_page(status, account_page(&session.account, &token, Some(message)))
}

fn account_page(account: &Account, token: &str, message: Option<Result<&str, String>>) -> Markup {
    pages::layout(
        "Your account",
        html! {
            h1 { (account.name) }
            p.meta {
                span { (if account.role == Role::Admin { "Admin" } else { "Contributor" }) }
            }
            p { "Your key: " code { (account.key) } }
            p.muted { "What you add is signed with this key, so other NEVOs can tell it came from you." }
            @match &message {
                Some(Ok(done)) => p.notice role="status" { (done) },
                Some(Err(error)) => p.error role="alert" { (error) },
                None => {},
            }
            ul {
                li { a href="/account/key" { "Copy your key to another device" } " (QR code)" }
                @if account.role == Role::Admin { li { a href="/accounts" { "Manage accounts" } } }
            }
            form method="post" action="/signout" {
                (csrf_field(token))
                button type="submit" { "Sign out" }
            }
            h2 { "Change your passphrase" }
            form method="post" action="/account/passphrase" {
                (csrf_field(token))
                label for="old" { "Current passphrase" }
                input type="password" id="old" name="old" required autocomplete="current-password";
                label for="new" { "New passphrase " span.hint { "(at least " (MIN_PASSPHRASE_CHARS) " characters)" } }
                input type="password" id="new" name="new" required autocomplete="new-password";
                label for="repeat" { "New passphrase again" }
                input type="password" id="repeat" name="repeat" required autocomplete="new-password";
                button type="submit" { "Change passphrase" }
            }
        },
    )
}

// ---------------------------------------------------------------------------------------------
// Key export (D-053)

async fn key_form(Contributor(session): Contributor) -> Response {
    form_page(StatusCode::OK, key_form_page(&session.csrf_token(), None))
}

/// Shows the secret key as a QR code, after the passphrase is typed again: a session alone
/// (a borrowed phone, a copied cookie) must not be enough to take someone's key away.
async fn key_export(
    State(state): State<AppState>,
    Contributor(session): Contributor,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    let form = Fields(form);
    let token = session.csrf_token();
    if let Err(e) = check_session_csrf(&session, form.get("csrf")) {
        return e.into_response();
    }
    let passphrase = Zeroizing::new(form.get("passphrase").to_string());
    let (accounts, id) = (state.accounts.clone(), session.account.id);
    match tokio::task::spawn_blocking(move || accounts.unlock_id(id, &passphrase)).await {
        Ok(Ok(key)) => {
            let text = accounts::secret_text(&key);
            match qr_svg(&text) {
                Ok(svg) => form_page(StatusCode::OK, key_page(&session.account, &text, &svg)),
                Err(e) => e.into_response(),
            }
        }
        Ok(Err(AccountError::WrongPassphrase)) => form_page(
            StatusCode::UNAUTHORIZED,
            key_form_page(&token, Some("Wrong passphrase.")),
        ),
        Ok(Err(e)) => AppError::from(e).into_response(),
        Err(e) => AppError::Internal(e.to_string()).into_response(),
    }
}

/// An inline SVG QR code. Inline SVG is markup, not a script or an image load, so the page's
/// strict content security policy allows it.
fn qr_svg(text: &str) -> AppResult<Zeroizing<String>> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)
        .map_err(|e| AppError::Internal(format!("QR code: {e}")))?;
    let svg = Zeroizing::new(
        code.render::<svg::Color>()
            .min_dimensions(240, 240)
            .dark_color(svg::Color("#000"))
            .light_color(svg::Color("#fff"))
            .build(),
    );
    // Drop the XML declaration, which doesn't belong inside an HTML page.
    let start = svg.find("<svg").unwrap_or(0);
    Ok(Zeroizing::new(svg[start..].to_string()))
}

fn key_form_page(token: &str, error: Option<&str>) -> Markup {
    pages::layout(
        "Your key",
        html! {
            h1 { "Copy your key to another device" }
            p { "Your secret key lets you sign as yourself on another device. Anyone who sees it can sign as you, so show it only to your own phone, with nobody looking." }
            @if let Some(error) = error { p.error role="alert" { (error) } }
            form method="post" action="/account/key" {
                (csrf_field(token))
                label for="passphrase" { "Type your passphrase again" }
                input type="password" id="passphrase" name="passphrase" required autocomplete="current-password";
                button type="submit" { "Show my secret key" }
            }
        },
    )
}

fn key_page(account: &Account, text: &str, svg: &str) -> Markup {
    pages::layout(
        "Your secret key",
        html! {
            h1 { "Your secret key" }
            p.notice { "Anyone who sees this can sign as " (account.name) ". Close this page when you are done." }
            div.qr { (PreEscaped(svg)) }
            p { "Scan it with the NEVO app on your other device, or copy the text:" }
            p { code.secret { (text) } }
            p.muted { "Public key: " (account.key) }
            p { a href="/account" { "Done" } }
        },
    )
}

// ---------------------------------------------------------------------------------------------
// Managing accounts (admins)

async fn list(State(state): State<AppState>, Admin(session): Admin) -> AppResult<Response> {
    let accounts = state.accounts.clone();
    let all = blocking(move || Ok(accounts.list()?)).await?;
    Ok(form_page(
        StatusCode::OK,
        accounts_page(&all, &session.account, &session.csrf_token(), &NewAccount::default()),
    ))
}

#[derive(Default)]
struct NewAccount {
    name: String,
    role: String,
    error: Option<String>,
}

async fn create(
    State(state): State<AppState>,
    Admin(session): Admin,
    Form(form): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let form = Fields(form);
    let token = session.csrf_token();
    check_session_csrf(&session, form.get("csrf"))?;
    let mut typed = NewAccount {
        name: form.get("name").trim().to_string(),
        role: form.get("role").to_string(),
        error: None,
    };
    let role = Role::parse(&typed.role).unwrap_or(Role::Contributor);
    let passphrase = Zeroizing::new(form.get("passphrase").to_string());
    let (store, node, accounts) = (state.store.clone(), state.node.clone(), state.accounts.clone());
    let name = typed.name.clone();
    let created = tokio::task::spawn_blocking(move || accounts.create(&store, &node, &name, &passphrase, role, false))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    match created {
        Ok(_) => Ok(Redirect::to("/accounts").into_response()),
        Err(e @ (AccountError::Invalid(_) | AccountError::NameTaken)) => {
            typed.error = Some(e.to_string());
            let accounts = state.accounts.clone();
            let all = blocking(move || Ok(accounts.list()?)).await?;
            Ok(form_page(
                StatusCode::BAD_REQUEST,
                accounts_page(&all, &session.account, &token, &typed),
            ))
        }
        Err(e) => Err(e.into()),
    }
}

async fn set_role(
    State(state): State<AppState>,
    Admin(session): Admin,
    Path(id): Path<i64>,
    Form(form): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let form = Fields(form);
    check_session_csrf(&session, form.get("csrf"))?;
    let role = Role::parse(form.get("role")).ok_or_else(|| AppError::BadRequest("Unknown role.".into()))?;
    let accounts = state.accounts.clone();
    blocking(move || Ok(accounts.set_role(id, role)?)).await?;
    Ok(Redirect::to("/accounts").into_response())
}

async fn set_active(
    State(state): State<AppState>,
    Admin(session): Admin,
    Path(id): Path<i64>,
    Form(form): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let form = Fields(form);
    check_session_csrf(&session, form.get("csrf"))?;
    let disabled = form.get("active") != "1";
    let accounts = state.accounts.clone();
    blocking(move || Ok(accounts.set_disabled(id, disabled)?)).await?;
    Ok(Redirect::to("/accounts").into_response())
}

fn accounts_page(all: &[Account], me: &Account, token: &str, typed: &NewAccount) -> Markup {
    pages::layout(
        "Accounts",
        html! {
            h1 { "Accounts" }
            p.muted { "Contributors can add guides. Admins can also manage accounts. Reading needs no account." }
            ul.items {
                @for account in all {
                    li {
                        strong { (account.name) }
                        @if account.id == me.id { " (you)" }
                        div.meta {
                            span { (if account.role == Role::Admin { "Admin" } else { "Contributor" }) }
                            @if account.disabled { span { "Turned off" } }
                            span { code { (pages::short_key(&account.key)) } }
                        }
                        form.inline method="post" action={ "/accounts/" (account.id) "/role" } {
                            (csrf_field(token))
                            @if account.role == Role::Admin {
                                input type="hidden" name="role" value="contributor";
                                button type="submit" { "Make contributor" }
                            } @else {
                                input type="hidden" name="role" value="admin";
                                button type="submit" { "Make admin" }
                            }
                        }
                        " "
                        form.inline method="post" action={ "/accounts/" (account.id) "/active" } {
                            (csrf_field(token))
                            @if account.disabled {
                                input type="hidden" name="active" value="1";
                                button type="submit" { "Turn on" }
                            } @else {
                                input type="hidden" name="active" value="0";
                                button type="submit" { "Turn off" }
                            }
                        }
                    }
                }
            }
            h2 { "New account" }
            p.muted { "Choose a first passphrase and tell it to the person. They can change it on their account page." }
            @if let Some(error) = &typed.error { p.error role="alert" { (error) } }
            form method="post" action="/accounts" {
                (csrf_field(token))
                label for="name" { "Name" }
                input type="text" id="name" name="name" required maxlength="64" value=(typed.name);
                label for="role" { "Role" }
                select id="role" name="role" {
                    option value="contributor" selected[typed.role != "admin"] { "Contributor: can add guides" }
                    option value="admin" selected[typed.role == "admin"] { "Admin: can also manage accounts" }
                }
                label for="passphrase" { "First passphrase " span.hint { "(at least " (MIN_PASSPHRASE_CHARS) " characters)" } }
                input type="password" id="passphrase" name="passphrase" required autocomplete="new-password";
                button type="submit" { "Create account" }
            }
        },
    )
}
