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

use store::{Ledger, NewLedger, NewVoucher, Register, RegisterFilter, Store, StoreError, Voucher};

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
        .route("/api/ledgers", get(list_ledgers).post(add_ledger))
        .route("/api/vouchers", get(list_vouchers).post(add_voucher))
        .route("/api/vouchers/{id}", get(get_voucher))
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024))
        .with_state(db)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::var_os("VOUCHER_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data/vouchers.duckdb"));
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let port = env::var("VOUCHER_PORT").map_or(Ok(3000), |port| port.parse::<u16>())?;
    let db = Arc::new(Mutex::new(Store::open(&path)?));
    let listener =
        tokio::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).await?;
    println!(
        "Voucher Desk is running at http://{}",
        listener.local_addr()?
    );
    println!("Database: {}", path.display());
    axum::serve(listener, router(db))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
