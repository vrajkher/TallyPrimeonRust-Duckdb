//! Sign-in, sessions, API tokens and the permission check for every API route.
//!
//! Until the first user exists, a new book behaves as before: requests from this
//! computer get full access and requests from elsewhere are refused. After that, every
//! API request needs a session cookie (with a CSRF header for changes) or an API token.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use axum::{
    Json,
    extract::{ConnectInfo, MatchedPath, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use ot_core::auth::{Grant, MIN_PASSWORD_LEN, Perm, Role, Scope, valid_username};
use ot_store::{
    StoreError,
    auth::{NewUserRecord, TokenInfo, UserInfo, UserUpdate},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::Database;

pub const SESSION_COOKIE: &str = "ot_session";
pub const CSRF_COOKIE: &str = "ot_csrf";
pub const CSRF_HEADER: &str = "x-csrf-token";
const SESSION_SECONDS: i64 = 12 * 60 * 60;
pub const TOKEN_PREFIX: &str = "ot_";
const MAX_FAILURES: u32 = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Shared state of the API: the book and the sign-in throttle.
#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub failures: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
    /// Add `Secure` to cookies (when served over HTTPS).
    pub secure_cookies: bool,
}

impl AppState {
    pub fn new(db: Database, secure_cookies: bool) -> Self {
        Self {
            db,
            failures: Arc::default(),
            secure_cookies,
        }
    }
}

/// Who is making a request and what they may do.
#[derive(Debug, Clone)]
pub struct Identity {
    pub user: Option<UserInfo>,
    pub grant: Grant,
}

impl Identity {
    /// The name written into voucher history.
    pub fn actor(&self) -> String {
        match &self.user {
            Some(user) => format!("{} ({})", user.full_name, user.username),
            None => "Local operator".into(),
        }
    }

    pub fn can(&self, doctype: &str, perm: Perm) -> bool {
        self.grant.can(doctype, perm)
    }

    pub fn require(&self, doctype: &str, perm: Perm) -> Result<(), AuthError> {
        if self.can(doctype, perm) {
            Ok(())
        } else {
            Err(AuthError::Forbidden)
        }
    }
}

#[derive(Debug)]
pub enum AuthError {
    SignIn,
    Forbidden,
    Csrf,
    TooManyAttempts,
    Invalid(String),
    Store(StoreError),
}

impl From<StoreError> for AuthError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::SignIn => (StatusCode::UNAUTHORIZED, "Sign in to OpenTally.".to_owned()),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "Your role does not allow this action.".to_owned(),
            ),
            Self::Csrf => (
                StatusCode::FORBIDDEN,
                "The request is missing its security token. Reload the page and try again."
                    .to_owned(),
            ),
            Self::TooManyAttempts => (
                StatusCode::TOO_MANY_REQUESTS,
                "Too many failed sign-ins. Wait 15 minutes and try again.".to_owned(),
            ),
            Self::Invalid(message) => (StatusCode::BAD_REQUEST, message),
            Self::Store(error) => return crate::ApiError(error).into_response(),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

// ---------------------------------------------------------------- secrets

pub fn random_hex(bytes: usize) -> Result<String, AuthError> {
    let mut buffer = vec![0_u8; bytes];
    getrandom::fill(&mut buffer).map_err(|_| AuthError::Store(StoreError::Unavailable))?;
    Ok(buffer.iter().map(|b| format!("{b:02x}")).collect())
}

/// Session and API tokens are stored only as this hash.
pub fn token_hash(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn hash_password(password: &str) -> Result<String, AuthError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(AuthError::Invalid(format!(
            "Use a password of at least {MIN_PASSWORD_LEN} characters."
        )));
    }
    let mut salt = [0_u8; 16];
    getrandom::fill(&mut salt).map_err(|_| AuthError::Store(StoreError::Unavailable))?;
    let salt =
        SaltString::encode_b64(&salt).map_err(|_| AuthError::Store(StoreError::Unavailable))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AuthError::Store(StoreError::Unavailable))
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// A request straight from this computer, not relayed by a proxy.
fn is_local(peer: Option<SocketAddr>, headers: &HeaderMap) -> bool {
    let relayed = ["forwarded", "x-forwarded-for", "x-real-ip"]
        .iter()
        .any(|name| headers.contains_key(*name));
    !relayed && peer.is_some_and(|peer| peer.ip().is_loopback())
}

// ---------------------------------------------------------------- identity

/// Finds who is calling. `None` means the request must sign in.
async fn identify(
    state: &AppState,
    method: &Method,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> Result<Option<Identity>, AuthError> {
    if let Some(secret) = bearer(headers) {
        let hash = token_hash(secret);
        let found = state.db.read(move |store| store.token(&hash)).await?;
        return Ok(found.map(|(user, scope)| Identity {
            grant: Grant::new(user.roles.clone(), Some(scope)),
            user: Some(user),
        }));
    }
    if let Some(secret) = cookie(headers, SESSION_COOKIE) {
        let hash = token_hash(secret);
        if let Some(session) = state.db.read(move |store| store.session(&hash)).await? {
            if !matches!(*method, Method::GET | Method::HEAD) {
                let sent = headers
                    .get(CSRF_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default();
                if !same(sent, &session.csrf_token) {
                    return Err(AuthError::Csrf);
                }
            }
            return Ok(Some(Identity {
                grant: Grant::new(session.user.roles.clone(), None),
                user: Some(session.user),
            }));
        }
    }
    let users = state.db.read(|store| store.user_count()).await?;
    if users == 0 && is_local(peer, headers) {
        // A book with no users yet: this computer keeps full access, as before.
        return Ok(Some(Identity {
            user: None,
            grant: Grant::new(vec![Role::SystemManager], None),
        }));
    }
    Ok(None)
}

/// What a route needs. Unlisted API routes are refused.
enum Rule {
    /// Any of these (DocType, permission) pairs.
    Any(&'static [(&'static str, Perm)]),
    /// The handler checks the voucher's kind itself.
    Handler,
    /// Any signed-in caller.
    Signed,
    /// A System Manager without a narrowing token scope.
    Admin,
}

const ACCOUNT: &str = "Books Account";
const JOURNAL: &str = "Books Journal Entry";
const PAYMENT: &str = "Books Payment";
const LEDGER: &str = "Books Ledger Entry";
const SETTINGS: &str = "Books Accounting Settings";
const SERIES: &str = "Books Number Series";
const PARTY: &str = "Books Party";
const ITEM: &str = "Books Item";
const ITEM_GROUP: &str = "Books Item Group";
const UNIT: &str = "Books Uom";

fn rule(method: &Method, path: &str) -> Option<Rule> {
    let path = path.strip_prefix("/api/v1").map_or(path, |rest| rest);
    let path = path.strip_prefix("/api").unwrap_or(path);
    let read = matches!(*method, Method::GET | Method::HEAD);
    Some(match (read, path) {
        // Payment entry needs the ledger list even without account rights, as a link field.
        (true, "/ledgers" | "/groups") => {
            Rule::Any(&[(ACCOUNT, Perm::Read), (PAYMENT, Perm::Create)])
        }
        (false, "/ledgers" | "/groups") => Rule::Any(&[(ACCOUNT, Perm::Create)]),
        (false, "/ledgers/{id}/openings") => Rule::Any(&[(ACCOUNT, Perm::Write)]),
        (true, "/vouchers" | "/vouchers/{id}" | "/vouchers/{id}/changes")
        | (true, "/vouchers/{id}/changes/{rev}") => {
            Rule::Any(&[(JOURNAL, Perm::Read), (PAYMENT, Perm::Read)])
        }
        (false, "/vouchers" | "/vouchers/{id}/changes") => Rule::Handler,
        (false, "/import") => Rule::Any(&[(JOURNAL, Perm::Import)]),
        (true, "/settings" | "/settings/accounting") => Rule::Any(&[(SETTINGS, Perm::Read)]),
        (false, "/settings" | "/settings/close-period" | "/settings/accounting") => {
            Rule::Any(&[(SETTINGS, Perm::Write)])
        }
        (true, "/accounts" | "/accounts/charts") => Rule::Any(&[(ACCOUNT, Perm::Read)]),
        (false, "/accounts/chart") => Rule::Any(&[(ACCOUNT, Perm::Create)]),
        (false, "/groups/{id}/root-type" | "/ledgers/{id}/account") => {
            Rule::Any(&[(ACCOUNT, Perm::Write)])
        }
        (true, p) if p.starts_with("/reports/") => Rule::Any(&[(LEDGER, Perm::Report)]),
        (true, "/bank/{id}") => Rule::Any(&[(LEDGER, Perm::Read)]),
        (false, "/bank/{id}" | "/bank/{id}/matches" | "/bank/matches/{id}/undo") => {
            Rule::Any(&[(JOURNAL, Perm::Submit)])
        }
        (_, "/session/password" | "/tokens" | "/tokens/{id}") => Rule::Signed,
        // Each MCP tool is checked against its own route by the MCP server.
        (_, p) if p == "/mcp" || p.starts_with("/mcp/") => Rule::Signed,
        (_, "/users" | "/users/{id}") => Rule::Admin,
        (true, "/series" | "/series/types") => Rule::Any(&[(SERIES, Perm::Read)]),
        (false, "/series") => Rule::Any(&[(SERIES, Perm::Create)]),
        (false, "/series/{prefix}") if *method == Method::DELETE => {
            Rule::Any(&[(SERIES, Perm::Delete)])
        }
        (false, "/series/{prefix}") => Rule::Any(&[(SERIES, Perm::Write)]),
        (true, "/parties" | "/parties/{id}") => Rule::Any(&[(PARTY, Perm::Read)]),
        (false, "/parties") => Rule::Any(&[(PARTY, Perm::Create)]),
        (false, "/parties/{id}") if *method == Method::DELETE => {
            Rule::Any(&[(PARTY, Perm::Delete)])
        }
        (false, "/parties/{id}") => Rule::Any(&[(PARTY, Perm::Write)]),
        (true, "/items" | "/items/{id}") => Rule::Any(&[(ITEM, Perm::Read)]),
        (false, "/items") => Rule::Any(&[(ITEM, Perm::Create)]),
        (false, "/items/{id}") if *method == Method::DELETE => Rule::Any(&[(ITEM, Perm::Delete)]),
        (false, "/items/{id}") => Rule::Any(&[(ITEM, Perm::Write)]),
        (true, "/item-groups") => Rule::Any(&[(ITEM_GROUP, Perm::Read)]),
        (false, "/item-groups") => Rule::Any(&[(ITEM_GROUP, Perm::Create)]),
        (false, "/item-groups/{id}") if *method == Method::DELETE => {
            Rule::Any(&[(ITEM_GROUP, Perm::Delete)])
        }
        (false, "/item-groups/{id}") => Rule::Any(&[(ITEM_GROUP, Perm::Write)]),
        (true, "/units") => Rule::Any(&[(UNIT, Perm::Read)]),
        (false, "/units") => Rule::Any(&[(UNIT, Perm::Create)]),
        (false, "/units/{name}") => Rule::Any(&[(UNIT, Perm::Delete)]),
        (true, "/company/countries" | "/company/defaults" | "/regional/indian-states") => {
            Rule::Signed
        }
        (false, "/company/setup") => Rule::Admin,
        _ => return None,
    })
}

/// Middleware for the protected API: identifies the caller and checks the route rule.
pub async fn guard(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let identity = identify(&state, request.method(), request.headers(), Some(peer))
        .await?
        .ok_or(AuthError::SignIn)?;
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_default();
    if !route_allowed(&identity, request.method(), &path) {
        return Err(AuthError::Forbidden);
    }
    request.extensions_mut().insert(identity);
    Ok(next.run(request).await)
}

/// Whether the caller may use a route. Routes whose rule is checked by the handler
/// (voucher posting and changes) pass here; the handler checks the voucher's kind.
pub fn route_allowed(identity: &Identity, method: &Method, path: &str) -> bool {
    match rule(method, path) {
        Some(Rule::Any(pairs)) => pairs
            .iter()
            .any(|(doctype, perm)| identity.can(doctype, *perm)),
        Some(Rule::Handler | Rule::Signed) => true,
        Some(Rule::Admin) => identity.grant.is_admin(),
        None => false,
    }
}

/// The DocType that governs a voucher kind.
pub fn voucher_doctype(kind: &str) -> &'static str {
    match kind {
        "payment" | "receipt" => PAYMENT,
        _ => JOURNAL,
    }
}

// ---------------------------------------------------------------- session routes

#[derive(Serialize)]
pub struct SessionState {
    signed_in: bool,
    user: Option<UserInfo>,
    /// No users exist yet; the first one is created with `POST /api/v1/setup`.
    setup_required: bool,
    csrf_token: Option<String>,
    /// The company setup wizard has run (`POST /api/v1/company/setup`).
    company_setup_complete: bool,
}

pub async fn session_state(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SessionState>, AuthError> {
    let users = state.db.read(|store| store.user_count()).await?;
    let company_setup_complete = state
        .db
        .read(|store| store.accounting_settings())
        .await?
        .setup_complete;
    let session = match cookie(&headers, SESSION_COOKIE) {
        Some(secret) => {
            let hash = token_hash(secret);
            state.db.read(move |store| store.session(&hash)).await?
        }
        None => None,
    };
    Ok(Json(SessionState {
        signed_in: session.is_some(),
        csrf_token: session.as_ref().map(|s| s.csrf_token.clone()),
        user: session.map(|s| s.user),
        setup_required: users == 0,
        company_setup_complete,
    }))
}

#[derive(Deserialize)]
pub struct SignIn {
    username: String,
    password: String,
}

pub async fn sign_in(
    State(state): State<AppState>,
    Json(input): Json<SignIn>,
) -> Result<Response, AuthError> {
    let username = input.username.trim().to_owned();
    check_throttle(&state, &username)?;
    let lookup = username.clone();
    let found = state
        .db
        .read(move |store| store.credentials(&lookup))
        .await?;
    let password = input.password;
    let verified = tokio::task::spawn_blocking(move || match found {
        Some((user, hash)) if user.enabled && verify_password(&password, &hash) => Some(user),
        Some(_) => None,
        None => {
            // Spend the same effort for unknown names.
            let _ = verify_password(&password, dummy_hash());
            None
        }
    })
    .await
    .map_err(|_| AuthError::Store(StoreError::Unavailable))?;
    match verified {
        Some(user) => {
            clear_failures(&state, &username);
            start_session(&state, user).await
        }
        None => {
            record_failure(&state, &username);
            Err(AuthError::Invalid(
                "The user name or password is not correct.".into(),
            ))
        }
    }
}

/// An Argon2 hash of a random password, so unknown user names cost the same time.
fn dummy_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| {
        let password = random_hex(16).unwrap_or_else(|_| "unused-password".into());
        hash_password(&password).unwrap_or_default()
    })
}

async fn start_session(state: &AppState, user: UserInfo) -> Result<Response, AuthError> {
    let secret = random_hex(32)?;
    let csrf = random_hex(16)?;
    let hash = token_hash(&secret);
    let (user_id, csrf_value) = (user.id, csrf.clone());
    state
        .db
        .write(move |store| {
            store.purge_expired_sessions()?;
            store.create_session(&hash, user_id, &csrf_value, SESSION_SECONDS)
        })
        .await?;
    let secure = if state.secure_cookies { "; Secure" } else { "" };
    let company_setup_complete = state
        .db
        .read(|store| store.accounting_settings())
        .await?
        .setup_complete;
    let mut response = Json(SessionState {
        signed_in: true,
        user: Some(user),
        setup_required: false,
        csrf_token: Some(csrf.clone()),
        company_setup_complete,
    })
    .into_response();
    let headers = response.headers_mut();
    headers.append(
        header::SET_COOKIE,
        header_value(format!(
            "{SESSION_COOKIE}={secret}; HttpOnly; SameSite=Strict; Path=/; Max-Age={SESSION_SECONDS}{secure}"
        ))?,
    );
    headers.append(
        header::SET_COOKIE,
        header_value(format!(
            "{CSRF_COOKIE}={csrf}; SameSite=Strict; Path=/; Max-Age={SESSION_SECONDS}{secure}"
        ))?,
    );
    Ok(response)
}

fn header_value(text: String) -> Result<HeaderValue, AuthError> {
    HeaderValue::from_str(&text).map_err(|_| AuthError::Store(StoreError::Unavailable))
}

pub async fn sign_out(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    if let Some(secret) = cookie(&headers, SESSION_COOKIE) {
        let hash = token_hash(secret);
        state
            .db
            .write(move |store| store.delete_session(&hash))
            .await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    for name in [SESSION_COOKIE, CSRF_COOKIE] {
        response.headers_mut().append(
            header::SET_COOKIE,
            header_value(format!("{name}=; SameSite=Strict; Path=/; Max-Age=0"))?,
        );
    }
    Ok(response)
}

fn check_throttle(state: &AppState, username: &str) -> Result<(), AuthError> {
    let mut failures = state
        .failures
        .lock()
        .map_err(|_| AuthError::TooManyAttempts)?;
    failures.retain(|_, (_, since)| since.elapsed() < FAILURE_WINDOW);
    match failures.get(username) {
        Some((count, _)) if *count >= MAX_FAILURES => Err(AuthError::TooManyAttempts),
        _ => Ok(()),
    }
}

fn record_failure(state: &AppState, username: &str) {
    if let Ok(mut failures) = state.failures.lock() {
        let entry = failures
            .entry(username.to_owned())
            .or_insert((0, Instant::now()));
        entry.0 += 1;
    }
}

fn clear_failures(state: &AppState, username: &str) {
    if let Ok(mut failures) = state.failures.lock() {
        failures.remove(username);
    }
}

#[derive(Deserialize)]
pub struct NewUser {
    username: String,
    full_name: String,
    password: String,
    #[serde(default)]
    roles: Vec<Role>,
}

fn user_record(input: NewUser, roles: Vec<Role>) -> Result<NewUserRecord, AuthError> {
    let username = input.username.trim().to_owned();
    if !valid_username(&username) {
        return Err(AuthError::Invalid(
            "Use 1 to 64 letters, digits, dots, hyphens, underscores or @ for the user name."
                .into(),
        ));
    }
    let full_name = input.full_name.trim().to_owned();
    if full_name.is_empty() || full_name.chars().count() > 100 {
        return Err(AuthError::Invalid(
            "Enter a name of 1 to 100 characters.".into(),
        ));
    }
    Ok(NewUserRecord {
        username,
        full_name,
        password_hash: hash_password(&input.password)?,
        roles,
    })
}

/// Creates the first user, a System Manager, from this computer, and signs them in.
pub async fn setup(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<NewUser>,
) -> Result<Response, AuthError> {
    if !is_local(Some(peer), &headers) {
        return Err(AuthError::Forbidden);
    }
    let record = tokio::task::spawn_blocking(move || user_record(input, vec![Role::SystemManager]))
        .await
        .map_err(|_| AuthError::Store(StoreError::Unavailable))??;
    let user = state
        .db
        .write(move |store| {
            if store.user_count()? > 0 {
                return Err(StoreError::Conflict(
                    "OpenTally already has users. Sign in instead.".into(),
                ));
            }
            store.create_user(record)
        })
        .await?;
    start_session(&state, user).await
}

#[derive(Deserialize)]
pub struct PasswordChange {
    current_password: String,
    new_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Json(input): Json<PasswordChange>,
) -> Result<StatusCode, AuthError> {
    let user = identity.user.ok_or(AuthError::SignIn)?;
    if identity.grant.scope.is_some() {
        return Err(AuthError::Forbidden);
    }
    let username = user.username.clone();
    let (_, hash) = state
        .db
        .read(move |store| store.credentials(&username))
        .await?
        .ok_or(AuthError::SignIn)?;
    let new_hash = tokio::task::spawn_blocking(move || {
        if verify_password(&input.current_password, &hash) {
            hash_password(&input.new_password)
        } else {
            Err(AuthError::Invalid(
                "The current password is not correct.".into(),
            ))
        }
    })
    .await
    .map_err(|_| AuthError::Store(StoreError::Unavailable))??;
    state
        .db
        .write(move |store| {
            store.update_user(
                user.id,
                UserUpdate {
                    password_hash: Some(new_hash),
                    ..Default::default()
                },
            )
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- users (admin)

pub async fn list_users(State(state): State<AppState>) -> Result<Json<Vec<UserInfo>>, AuthError> {
    Ok(Json(state.db.read(|store| store.users()).await?))
}

pub async fn add_user(
    State(state): State<AppState>,
    Json(input): Json<NewUser>,
) -> Result<(StatusCode, Json<UserInfo>), AuthError> {
    let roles = input.roles.clone();
    let record = tokio::task::spawn_blocking(move || user_record(input, roles))
        .await
        .map_err(|_| AuthError::Store(StoreError::Unavailable))??;
    let user = state
        .db
        .write(move |store| store.create_user(record))
        .await?;
    Ok((StatusCode::CREATED, Json(user)))
}

#[derive(Deserialize)]
pub struct ChangeUser {
    full_name: Option<String>,
    enabled: Option<bool>,
    roles: Option<Vec<Role>>,
    password: Option<String>,
}

pub async fn change_user(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<ChangeUser>,
) -> Result<Json<UserInfo>, AuthError> {
    let password_hash = match input.password {
        Some(password) => Some(
            tokio::task::spawn_blocking(move || hash_password(&password))
                .await
                .map_err(|_| AuthError::Store(StoreError::Unavailable))??,
        ),
        None => None,
    };
    let full_name = match input.full_name.map(|name| name.trim().to_owned()) {
        Some(name) if name.is_empty() || name.chars().count() > 100 => {
            return Err(AuthError::Invalid(
                "Enter a name of 1 to 100 characters.".into(),
            ));
        }
        other => other,
    };
    let update = UserUpdate {
        full_name,
        enabled: input.enabled,
        roles: input.roles,
        password_hash,
    };
    Ok(Json(
        state
            .db
            .write(move |store| store.update_user(id, update))
            .await?,
    ))
}

// ---------------------------------------------------------------- API tokens

#[derive(Deserialize)]
pub struct NewToken {
    name: String,
    scope: Scope,
}

#[derive(Serialize)]
pub struct CreatedToken {
    #[serde(flatten)]
    info: TokenInfo,
    /// Shown once; only its hash is stored.
    secret: String,
}

pub async fn add_token(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Json(input): Json<NewToken>,
) -> Result<(StatusCode, Json<CreatedToken>), AuthError> {
    let user = identity.user.clone().ok_or(AuthError::Forbidden)?;
    // A token cannot outrank the credential that creates it.
    if identity
        .grant
        .scope
        .is_some_and(|scope| input.scope > scope)
        || (input.scope == Scope::Admin && !user.roles.contains(&Role::SystemManager))
    {
        return Err(AuthError::Forbidden);
    }
    let secret = format!("{TOKEN_PREFIX}{}", random_hex(32)?);
    let hash = token_hash(&secret);
    let info = state
        .db
        .write(move |store| store.create_token(user.id, &input.name, &hash, input.scope))
        .await?;
    Ok((StatusCode::CREATED, Json(CreatedToken { info, secret })))
}

#[derive(Deserialize)]
pub struct TokenFilter {
    #[serde(default)]
    all: bool,
}

pub async fn list_tokens(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Query(filter): Query<TokenFilter>,
) -> Result<Json<Vec<TokenInfo>>, AuthError> {
    let owner = match (&identity.user, filter.all) {
        (_, true) if identity.grant.is_admin() => None,
        (Some(user), false) => Some(user.id),
        _ => return Err(AuthError::Forbidden),
    };
    Ok(Json(state.db.read(move |store| store.tokens(owner)).await?))
}

pub async fn revoke_token(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<i64>,
) -> Result<Json<TokenInfo>, AuthError> {
    let owner = if identity.grant.is_admin() {
        None
    } else {
        Some(identity.user.as_ref().ok_or(AuthError::Forbidden)?.id)
    };
    Ok(Json(
        state
            .db
            .write(move |store| store.revoke_token(id, owner))
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_hash_and_verify() {
        assert!(matches!(hash_password("short"), Err(AuthError::Invalid(_))));
        let hash = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("wrong horse battery", &hash));
        assert!(!verify_password("anything", "not a hash"));
        assert!(PasswordHash::new(dummy_hash()).is_ok());
    }

    #[test]
    fn cookies_bearer_and_locality() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "a=1; ot_session=abc; ot_csrf=x".parse().unwrap(),
        );
        headers.insert(header::AUTHORIZATION, "Bearer ot_123".parse().unwrap());
        assert_eq!(cookie(&headers, SESSION_COOKIE), Some("abc"));
        assert_eq!(bearer(&headers), Some("ot_123"));
        let local: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let remote: SocketAddr = "192.168.1.9:5000".parse().unwrap();
        assert!(is_local(Some(local), &headers));
        assert!(!is_local(Some(remote), &headers));
        assert!(!is_local(None, &headers));
        headers.insert("x-forwarded-for", "203.0.113.5".parse().unwrap());
        assert!(
            !is_local(Some(local), &headers),
            "a proxied request is not local"
        );
        assert_eq!(token_hash("x").len(), 64);
        assert!(same("abc", "abc") && !same("abc", "abd") && !same("abc", "ab"));
    }

    #[test]
    fn every_api_route_has_a_rule() {
        for (method, path) in [
            (Method::GET, "/api/ledgers"),
            (Method::POST, "/api/ledgers"),
            (Method::GET, "/api/v1/vouchers/{id}/changes/{rev}"),
            (Method::POST, "/api/vouchers"),
            (Method::POST, "/api/import"),
            (Method::POST, "/api/settings/close-period"),
            (Method::GET, "/api/reports/trial-balance"),
            (Method::POST, "/api/bank/matches/{id}/undo"),
            (Method::GET, "/api/v1/users"),
            (Method::DELETE, "/api/v1/tokens/{id}"),
            (Method::GET, "/api/v1/accounts"),
            (Method::POST, "/api/v1/accounts/chart"),
            (Method::POST, "/api/v1/ledgers/{id}/account"),
            (Method::POST, "/api/v1/settings/accounting"),
        ] {
            assert!(rule(&method, path).is_some(), "{method} {path}");
        }
        assert!(rule(&Method::GET, "/api/unknown").is_none());
    }
}
