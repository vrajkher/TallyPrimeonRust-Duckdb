#[cfg(test)]
mod api_tests;
mod auth;
mod db;

use std::{
    env, fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

use axum::{
    Extension, Json, Router,
    extract::{FromRef, Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
};
use ot_core::auth::Perm;
use serde::Serialize;

use ot_store::bank::{
    MatchRequest, MatchResult, Review, StatementImport, StatementImportResult, UndoMatch,
};
use ot_store::imports::{ImportRequest, ImportSummary};
use ot_store::reports::{
    AccountGroup, BookSettings, ClosePeriod, CompanyName, CostFilter, CostTotal, NewGroup, Opening,
    Period, Statement, StatementLine, TrialBalance,
};
use ot_store::{
    Ledger, NewLedger, NewVoucher, Register, RegisterFilter, Store, StoreError, Voucher,
    VoucherChange, VoucherEvent,
};

use auth::{AppState, AuthError, Identity};
use db::Database;

impl FromRef<AppState> for Database {
    fn from_ref(state: &AppState) -> Self {
        state.db.clone()
    }
}
type ApiResult<T> = Result<T, ApiError>;

struct ApiError(StoreError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct ErrorBody {
            error: String,
        }

        let (status, error) = match self.0 {
            StoreError::Invalid(message) => (StatusCode::BAD_REQUEST, message),
            StoreError::Conflict(message) => (StatusCode::CONFLICT, message),
            StoreError::Missing(message) => (StatusCode::NOT_FOUND, message),
            StoreError::Database(error) => {
                eprintln!("DuckDB error: {error}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "The local database could not complete the request.".to_owned(),
                )
            }
            StoreError::Unavailable => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "The local database is unavailable.".to_owned(),
            ),
        };
        (status, Json(ErrorBody { error })).into_response()
    }
}

async fn list_ledgers(State(db): State<Database>) -> ApiResult<Json<Vec<Ledger>>> {
    db.read(|store| store.ledgers())
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn add_ledger(
    State(db): State<Database>,
    Json(input): Json<NewLedger>,
) -> ApiResult<(StatusCode, Json<Ledger>)> {
    let ledger = db
        .write(|store| store.add_ledger(input))
        .await
        .map_err(ApiError)?;
    Ok((StatusCode::CREATED, Json(ledger)))
}

async fn list_vouchers(
    State(db): State<Database>,
    Query(filter): Query<RegisterFilter>,
) -> ApiResult<Json<Register>> {
    db.read(|store| store.register(filter))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn get_voucher(State(db): State<Database>, Path(id): Path<i64>) -> ApiResult<Json<Voucher>> {
    db.read(move |store| store.voucher(id))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn add_voucher(
    State(db): State<Database>,
    Extension(identity): Extension<Identity>,
    Json(input): Json<NewVoucher>,
) -> Result<(StatusCode, Json<Voucher>), AuthError> {
    identity.require(auth::voucher_doctype(input.kind_key()), Perm::Submit)?;
    let actor = identity.actor();
    let (voucher, created) = db
        .write(move |store| store.add_voucher_by(input, &actor))
        .await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(voucher)))
}

async fn revise_voucher(
    State(db): State<Database>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<i64>,
    Json(mut input): Json<VoucherChange>,
) -> Result<Json<Voucher>, AuthError> {
    // Editing needs Amend on both the current and the new kind; cancelling needs Cancel.
    let current = db.read(move |store| store.voucher(id)).await?;
    match input.replacement() {
        Some(replacement) => {
            identity.require(auth::voucher_doctype(current.kind()), Perm::Amend)?;
            identity.require(auth::voucher_doctype(replacement.kind_key()), Perm::Amend)?;
        }
        None => identity.require(auth::voucher_doctype(current.kind()), Perm::Cancel)?,
    }
    if identity.user.is_some() {
        input.set_actor(identity.actor());
    }
    Ok(Json(
        db.write(move |store| store.change_voucher(id, input))
            .await?,
    ))
}

async fn history(
    State(db): State<Database>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<VoucherEvent>>> {
    db.read(move |store| store.history(id))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn revision(
    State(db): State<Database>,
    Path((id, rev)): Path<(i64, i32)>,
) -> ApiResult<Json<VoucherEvent>> {
    db.read(move |store| store.history_revision(id, rev))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn import_book(
    State(db): State<Database>,
    Json(input): Json<ImportRequest>,
) -> ApiResult<Json<ImportSummary>> {
    db.write(|store| store.import(input))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn groups(State(db): State<Database>) -> ApiResult<Json<Vec<AccountGroup>>> {
    db.read(|store| store.groups())
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn add_group(
    State(db): State<Database>,
    Json(input): Json<NewGroup>,
) -> ApiResult<Json<AccountGroup>> {
    db.write(|store| store.add_group(input))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn settings(State(db): State<Database>) -> ApiResult<Json<BookSettings>> {
    db.read(|store| store.settings())
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn name_company(
    State(db): State<Database>,
    Json(input): Json<CompanyName>,
) -> ApiResult<Json<BookSettings>> {
    db.write(|store| store.name_company(input))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn close_period(
    State(db): State<Database>,
    Json(input): Json<ClosePeriod>,
) -> ApiResult<Json<BookSettings>> {
    db.write(|store| store.close_period(input))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn opening(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Json(input): Json<Opening>,
) -> ApiResult<Json<()>> {
    db.write(move |store| store.record_opening(id, input))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn trial_balance(
    State(db): State<Database>,
    Query(period): Query<Period>,
) -> ApiResult<Json<TrialBalance>> {
    db.read(|store| store.trial_balance(period))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn statement(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Query(period): Query<Period>,
) -> ApiResult<Json<Statement>> {
    db.read(move |store| store.statement(id, period))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn cost_totals(
    State(db): State<Database>,
    Query(period): Query<Period>,
) -> ApiResult<Json<Vec<CostTotal>>> {
    db.read(|store| store.cost_totals(period))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn cost_postings(
    State(db): State<Database>,
    Query(filter): Query<CostFilter>,
) -> ApiResult<Json<Vec<StatementLine>>> {
    db.read(|store| store.cost_postings(filter))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn bank_import(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Json(input): Json<StatementImport>,
) -> ApiResult<Json<StatementImportResult>> {
    db.write(move |store| store.import_statement(id, input))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn bank_review(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Query(period): Query<Period>,
) -> ApiResult<Json<Review>> {
    db.read(move |store| store.reconciliation(id, period))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn bank_match(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Json(input): Json<MatchRequest>,
) -> ApiResult<Json<MatchResult>> {
    db.write(move |store| store.reconcile(id, input))
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn bank_undo(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Json(input): Json<UndoMatch>,
) -> ApiResult<Json<()>> {
    db.write(move |store| store.undo_reconciliation(id, input))
        .await
        .map(Json)
        .map_err(ApiError)
}

/// The book's API routes, served at both `/api` and `/api/v1`.
fn book_routes() -> Router<AppState> {
    Router::new()
        .route("/ledgers", get(list_ledgers).post(add_ledger))
        .route("/vouchers", get(list_vouchers).post(add_voucher))
        .route("/vouchers/{id}", get(get_voucher))
        .route("/vouchers/{id}/changes", get(history).post(revise_voucher))
        .route("/vouchers/{id}/changes/{rev}", get(revision))
        .route(
            "/import",
            axum::routing::post(import_book)
                .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/groups", get(groups).post(add_group))
        .route("/settings", get(settings).post(name_company))
        .route("/settings/close-period", axum::routing::post(close_period))
        .route("/ledgers/{id}/openings", axum::routing::post(opening))
        .route("/reports/trial-balance", get(trial_balance))
        .route("/reports/ledgers/{id}", get(statement))
        .route("/reports/cost-centres", get(cost_totals))
        .route("/reports/cost-centres/postings", get(cost_postings))
        .route("/bank/{id}", get(bank_review).post(bank_import))
        .route("/bank/{id}/matches", axum::routing::post(bank_match))
        .route("/bank/matches/{id}/undo", axum::routing::post(bank_undo))
        .route("/session/password", post(auth::change_password))
        .route("/users", get(auth::list_users).post(auth::add_user))
        .route("/users/{id}", post(auth::change_user))
        .route("/tokens", get(auth::list_tokens).post(auth::add_token))
        .route("/tokens/{id}", delete(auth::revoke_token))
}

/// Where the built Leptos interface is served from. `trunk build` in `crates/ot-web`
/// writes it to `crates/ot-web/dist`.
fn ui_dir() -> PathBuf {
    env::var_os("OPENTALLY_UI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("crates/ot-web/dist"))
}

const UI_MISSING: &str = "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>OpenTally</title>\
<body style=\"font-family:system-ui;max-width:40rem;margin:3rem auto;padding:0 1rem;line-height:1.5\">\
<h1>The new OpenTally interface is not built</h1>\
<p>Build it with <code>trunk build --release</code> in <code>crates/ot-web</code>, or set \
<code>OPENTALLY_UI_DIR</code> to a built copy. The <a href=\"/\">classic screens</a> keep working.</p></body></html>";

/// The Leptos interface at `/app/`. Unknown paths get its `index.html`.
fn ui_service(dir: PathBuf) -> Router<AppState> {
    let index = dir.join("index.html");
    if !index.is_file() {
        return Router::new()
            .route("/app", get(|| async { Html(UI_MISSING) }))
            .route("/app/", get(|| async { Html(UI_MISSING) }))
            .route("/app/{*rest}", get(|| async { Html(UI_MISSING) }));
    }
    let files = tower_http::services::ServeDir::new(dir)
        .fallback(tower_http::services::ServeFile::new(index));
    Router::new()
        .route(
            "/app",
            get(|| async { axum::response::Redirect::permanent("/app/") }),
        )
        .nest_service("/app/", files)
}

fn router(state: AppState) -> Router {
    router_with_ui(state, ui_dir())
}

fn router_with_ui(state: AppState, ui: PathBuf) -> Router {
    let protected = Router::new()
        .merge(Router::new().nest("/api", book_routes()))
        .nest("/api/v1", book_routes())
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::guard,
        ));
    let public = Router::new()
        .route(
            "/api/v1/session",
            get(auth::session_state)
                .post(auth::sign_in)
                .delete(auth::sign_out),
        )
        .route("/api/v1/setup", post(auth::setup));
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../../../web/index.html")) }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/style.css"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/app.js"),
                )
            }),
        )
        .route(
            "/books.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/books.js"),
                )
            }),
        )
        .merge(ui_service(ui))
        .merge(public)
        .merge(protected)
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::var_os("OPENTALLY_DB")
        .or_else(|| env::var_os("VOUCHER_DB"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data/vouchers.duckdb"));
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let port = env::var("OPENTALLY_PORT")
        .or_else(|_| env::var("VOUCHER_PORT"))
        .map_or(Ok(3000), |port| port.parse::<u16>())?;
    let bind_addr = match env::var("OPENTALLY_BIND_ADDR") {
        Ok(addr) => addr
            .parse::<IpAddr>()
            .map_err(|err| format!("Invalid OPENTALLY_BIND_ADDR {addr:?}: {err}"))?,
        Err(env::VarError::NotPresent) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        Err(err) => return Err(format!("Cannot read OPENTALLY_BIND_ADDR: {err}").into()),
    };
    if let Some(backup) = ot_store::migrate::backup_before_migrating(&path)? {
        println!(
            "Backed up the book to {} before upgrading it",
            backup.display()
        );
    }
    let db = Database::start(Store::open(&path)?)
        .map_err(|_| "The book's database worker could not start.")?;
    let secure_cookies = matches!(
        env::var("OPENTALLY_SECURE_COOKIES").as_deref(),
        Ok("1" | "true")
    );
    let users = db
        .read(|store| store.user_count())
        .await
        .map_err(|_| "The book's users could not be read.")?;
    if users == 0 && !bind_addr.is_loopback() {
        println!(
            "No users exist yet, so only requests from this computer are accepted. \
             Create the first user before opening OpenTally to the network."
        );
    }
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(bind_addr, port)).await?;
    println!("OpenTally is running at http://{}", listener.local_addr()?);
    println!("Database: {}", path.display());
    axum::serve(
        listener,
        router(AppState::new(db, secure_cookies))
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
