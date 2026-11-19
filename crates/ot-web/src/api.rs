//! Calls to the OpenTally `/api/v1` API from the browser.

use gloo_net::http::{Request, Response};
use ot_api_types::{ApiErrorBody, DayBook, DayBookFilter, SessionState, Setup, SignIn};
use serde::de::DeserializeOwned;

/// A failed call: `signed_out` asks the app to show the sign-in form again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub message: String,
    pub signed_out: bool,
}

impl ApiError {
    fn network() -> Self {
        Self {
            message: "Could not reach OpenTally. Check that it is running and try again.".into(),
            signed_out: false,
        }
    }
}

async fn read<T: DeserializeOwned>(response: Response) -> Result<T, ApiError> {
    if response.ok() {
        return response.json::<T>().await.map_err(|_| ApiError {
            message: "OpenTally sent a reply this page could not read.".into(),
            signed_out: false,
        });
    }
    let signed_out = response.status() == 401;
    let message = response
        .json::<ApiErrorBody>()
        .await
        .map(|body| body.error)
        .unwrap_or_else(|_| format!("OpenTally returned HTTP {}.", response.status()));
    Err(ApiError {
        message,
        signed_out,
    })
}

pub async fn session() -> Result<SessionState, ApiError> {
    let response = Request::get("/api/v1/session")
        .send()
        .await
        .map_err(|_| ApiError::network())?;
    read(response).await
}

pub async fn sign_in(input: &SignIn) -> Result<SessionState, ApiError> {
    let response = Request::post("/api/v1/session")
        .json(input)
        .map_err(|_| ApiError::network())?
        .send()
        .await
        .map_err(|_| ApiError::network())?;
    read(response).await
}

pub async fn setup(input: &Setup) -> Result<SessionState, ApiError> {
    let response = Request::post("/api/v1/setup")
        .json(input)
        .map_err(|_| ApiError::network())?
        .send()
        .await
        .map_err(|_| ApiError::network())?;
    read(response).await
}

pub async fn sign_out() {
    let _ = Request::delete("/api/v1/session").send().await;
}

pub async fn day_book(filter: &DayBookFilter) -> Result<DayBook, ApiError> {
    let query = filter.query();
    let url = if query.is_empty() {
        "/api/v1/vouchers".to_owned()
    } else {
        format!("/api/v1/vouchers?{query}")
    };
    let response = Request::get(&url)
        .send()
        .await
        .map_err(|_| ApiError::network())?;
    read(response).await
}
