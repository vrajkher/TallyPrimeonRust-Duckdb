#[cfg(test)]
mod acceptance;
mod bank;
mod imports;
mod reports;
mod store;

use std::{
    env, fs,
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use bank::{MatchRequest, MatchResult, Review, StatementImport, StatementImportResult, UndoMatch};
use imports::{ImportRequest, ImportSummary};
use reports::{
    AccountGroup, BookSettings, ClosePeriod, CompanyName, CostFilter, CostTotal, NewGroup, Opening,
    Period, Statement, StatementLine, TrialBalance,
};
use store::{
    Ledger, NewLedger, NewVoucher, Register, RegisterFilter, Store, StoreError, Voucher,
    VoucherChange, VoucherEvent,
};

type Database = Arc<Mutex<Store>>;
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

async fn run_db<T: Send + 'static>(
    db: Database,
    work: impl FnOnce(&mut Store) -> Result<T, StoreError> + Send + 'static,
) -> ApiResult<T> {
    tokio::task::spawn_blocking(move || {
        let mut store = db.lock().map_err(|_| StoreError::Unavailable)?;
        work(&mut store)
    })
    .await
    .map_err(|error| {
        eprintln!("Database worker error: {error}");
        ApiError(StoreError::Unavailable)
    })?
    .map_err(ApiError)
}

async fn list_ledgers(State(db): State<Database>) -> ApiResult<Json<Vec<Ledger>>> {
    run_db(db, |store| store.ledgers()).await.map(Json)
}

async fn add_ledger(
    State(db): State<Database>,
    Json(input): Json<NewLedger>,
) -> ApiResult<(StatusCode, Json<Ledger>)> {
    let ledger = run_db(db, |store| store.add_ledger(input)).await?;
    Ok((StatusCode::CREATED, Json(ledger)))
}

async fn list_vouchers(
    State(db): State<Database>,
    Query(filter): Query<RegisterFilter>,
) -> ApiResult<Json<Register>> {
    run_db(db, |store| store.register(filter)).await.map(Json)
}

async fn get_voucher(State(db): State<Database>, Path(id): Path<i64>) -> ApiResult<Json<Voucher>> {
    run_db(db, move |store| store.voucher(id)).await.map(Json)
}

async fn add_voucher(
    State(db): State<Database>,
    Json(input): Json<NewVoucher>,
) -> ApiResult<(StatusCode, Json<Voucher>)> {
    let (voucher, created) = run_db(db, |store| store.add_voucher(input)).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(voucher)))
}

async fn revise_voucher(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Json(input): Json<VoucherChange>,
) -> ApiResult<Json<Voucher>> {
    run_db(db, move |store| store.change_voucher(id, input))
        .await
        .map(Json)
}

async fn history(
    State(db): State<Database>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<VoucherEvent>>> {
    run_db(db, move |store| store.history(id)).await.map(Json)
}

async fn revision(
    State(db): State<Database>,
    Path((id, rev)): Path<(i64, i32)>,
) -> ApiResult<Json<VoucherEvent>> {
    run_db(db, move |store| store.history_revision(id, rev))
        .await
        .map(Json)
}

async fn import_book(
    State(db): State<Database>,
    Json(input): Json<ImportRequest>,
) -> ApiResult<Json<ImportSummary>> {
    run_db(db, |store| store.import(input)).await.map(Json)
}

async fn groups(State(db): State<Database>) -> ApiResult<Json<Vec<AccountGroup>>> {
    run_db(db, |store| store.groups()).await.map(Json)
}

async fn add_group(
    State(db): State<Database>,
    Json(input): Json<NewGroup>,
) -> ApiResult<Json<AccountGroup>> {
    run_db(db, |store| store.add_group(input)).await.map(Json)
}

async fn settings(State(db): State<Database>) -> ApiResult<Json<BookSettings>> {
    run_db(db, |store| store.settings()).await.map(Json)
}

async fn name_company(
    State(db): State<Database>,
    Json(input): Json<CompanyName>,
) -> ApiResult<Json<BookSettings>> {
    run_db(db, |store| store.name_company(input))
        .await
        .map(Json)
}

async fn close_period(
    State(db): State<Database>,
    Json(input): Json<ClosePeriod>,
) -> ApiResult<Json<BookSettings>> {
    run_db(db, |store| store.close_period(input))
        .await
        .map(Json)
}

async fn opening(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Json(input): Json<Opening>,
) -> ApiResult<Json<()>> {
    run_db(db, move |store| store.record_opening(id, input))
        .await
        .map(Json)
}

async fn trial_balance(
    State(db): State<Database>,
    Query(period): Query<Period>,
) -> ApiResult<Json<TrialBalance>> {
    run_db(db, |store| store.trial_balance(period))
        .await
        .map(Json)
}

async fn statement(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Query(period): Query<Period>,
) -> ApiResult<Json<Statement>> {
    run_db(db, move |store| store.statement(id, period))
        .await
        .map(Json)
}

async fn cost_totals(
    State(db): State<Database>,
    Query(period): Query<Period>,
) -> ApiResult<Json<Vec<CostTotal>>> {
    run_db(db, |store| store.cost_totals(period))
        .await
        .map(Json)
}

async fn cost_postings(
    State(db): State<Database>,
    Query(filter): Query<CostFilter>,
) -> ApiResult<Json<Vec<StatementLine>>> {
    run_db(db, |store| store.cost_postings(filter))
        .await
        .map(Json)
}

async fn bank_import(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Json(input): Json<StatementImport>,
) -> ApiResult<Json<StatementImportResult>> {
    run_db(db, move |store| store.import_statement(id, input))
        .await
        .map(Json)
}

async fn bank_review(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Query(period): Query<Period>,
) -> ApiResult<Json<Review>> {
    run_db(db, move |store| store.reconciliation(id, period))
        .await
        .map(Json)
}

async fn bank_match(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Json(input): Json<MatchRequest>,
) -> ApiResult<Json<MatchResult>> {
    run_db(db, move |store| store.reconcile(id, input))
        .await
        .map(Json)
}

async fn bank_undo(
    State(db): State<Database>,
    Path(id): Path<i64>,
    Json(input): Json<UndoMatch>,
) -> ApiResult<Json<()>> {
    run_db(db, move |store| store.undo_reconciliation(id, input))
        .await
        .map(Json)
}

fn router(db: Database) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/style.css"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route(
            "/books.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/books.js"),
                )
            }),
        )
        .route("/api/ledgers", get(list_ledgers).post(add_ledger))
        .route("/api/vouchers", get(list_vouchers).post(add_voucher))
        .route("/api/vouchers/{id}", get(get_voucher))
        .route(
            "/api/vouchers/{id}/changes",
            get(history).post(revise_voucher),
        )
        .route("/api/vouchers/{id}/changes/{rev}", get(revision))
        .route(
            "/api/import",
            axum::routing::post(import_book)
                .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/api/groups", get(groups).post(add_group))
        .route("/api/settings", get(settings).post(name_company))
        .route(
            "/api/settings/close-period",
            axum::routing::post(close_period),
        )
        .route("/api/ledgers/{id}/openings", axum::routing::post(opening))
        .route("/api/reports/trial-balance", get(trial_balance))
        .route("/api/reports/ledgers/{id}", get(statement))
        .route("/api/reports/cost-centres", get(cost_totals))
        .route("/api/reports/cost-centres/postings", get(cost_postings))
        .route("/api/bank/{id}", get(bank_review).post(bank_import))
        .route("/api/bank/{id}/matches", axum::routing::post(bank_match))
        .route(
            "/api/bank/matches/{id}/undo",
            axum::routing::post(bank_undo),
        )
        .with_state(db)
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
    let db = Arc::new(Mutex::new(Store::open(&path)?));
    let listener =
        tokio::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).await?;
    println!("OpenTally is running at http://{}", listener.local_addr()?);
    println!("Database: {}", path.display());
    axum::serve(listener, router(db))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
