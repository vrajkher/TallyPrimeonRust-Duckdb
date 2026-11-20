//! Calls to the OpenTally `/api/v1` API from the browser.

use gloo_net::http::{Request, Response};
use ot_api_types::{
    ApiErrorBody, ChartOfAccounts, ChartTemplate, CompanySetup, CountryDefaults, DayBook,
    DayBookFilter, Item, ItemGroup, ItemInput, NewNumberSeries, NumberSeries, Party, PartyInput,
    SeriesType, SessionState, Setup, SignIn, Tax, TaxInput, Unit,
};
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

/// The session's CSRF token from the `ot_csrf` cookie; empty in a book without users.
fn csrf_token() -> String {
    use leptos::wasm_bindgen::JsCast;
    let cookies = leptos::prelude::document()
        .dyn_into::<web_sys::HtmlDocument>()
        .ok()
        .and_then(|doc| doc.cookie().ok())
        .unwrap_or_default();
    cookies
        .split("; ")
        .find_map(|pair| pair.strip_prefix("ot_csrf="))
        .unwrap_or_default()
        .to_owned()
}

async fn get<T: DeserializeOwned>(url: &str) -> Result<T, ApiError> {
    let response = Request::get(url)
        .send()
        .await
        .map_err(|_| ApiError::network())?;
    read(response).await
}

/// A change: JSON body, sent with the session's CSRF token.
async fn post<B: serde::Serialize, T: DeserializeOwned>(
    url: &str,
    body: &B,
) -> Result<T, ApiError> {
    let response = Request::post(url)
        .header("x-csrf-token", &csrf_token())
        .json(body)
        .map_err(|_| ApiError::network())?
        .send()
        .await
        .map_err(|_| ApiError::network())?;
    read(response).await
}

/// Percent-encodes a path segment or query value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub async fn number_series() -> Result<Vec<NumberSeries>, ApiError> {
    get("/api/v1/series").await
}

pub async fn series_types() -> Result<Vec<SeriesType>, ApiError> {
    get("/api/v1/series/types").await
}

pub async fn add_number_series(input: &NewNumberSeries) -> Result<NumberSeries, ApiError> {
    post("/api/v1/series", input).await
}

pub async fn make_default_series(prefix: &str) -> Result<NumberSeries, ApiError> {
    post(
        &format!("/api/v1/series/{}", encode(prefix)),
        &serde_json::json!({"make_default": true}),
    )
    .await
}

pub async fn delete_number_series(prefix: &str) -> Result<(), ApiError> {
    delete(&format!("/api/v1/series/{}", encode(prefix))).await
}

/// A DELETE sent with the session's CSRF token; the reply has no body.
async fn delete(url: &str) -> Result<(), ApiError> {
    let response = Request::delete(url)
        .header("x-csrf-token", &csrf_token())
        .send()
        .await
        .map_err(|_| ApiError::network())?;
    if response.ok() {
        return Ok(());
    }
    read::<serde_json::Value>(response).await.map(|_| ())
}

pub async fn parties(role: &str, q: &str) -> Result<Vec<Party>, ApiError> {
    let mut query = Vec::new();
    if !role.is_empty() {
        query.push(format!("role={}", encode(role)));
    }
    if !q.trim().is_empty() {
        query.push(format!("q={}", encode(q.trim())));
    }
    get(&format!("/api/v1/parties?{}", query.join("&"))).await
}

pub async fn save_party(id: Option<i64>, input: &PartyInput) -> Result<Party, ApiError> {
    match id {
        Some(id) => post(&format!("/api/v1/parties/{id}"), input).await,
        None => post("/api/v1/parties", input).await,
    }
}

pub async fn delete_party(id: i64) -> Result<(), ApiError> {
    delete(&format!("/api/v1/parties/{id}")).await
}

pub async fn items(q: &str) -> Result<Vec<Item>, ApiError> {
    if q.trim().is_empty() {
        get("/api/v1/items").await
    } else {
        get(&format!("/api/v1/items?q={}", encode(q.trim()))).await
    }
}

pub async fn save_item(id: Option<i64>, input: &ItemInput) -> Result<Item, ApiError> {
    match id {
        Some(id) => post(&format!("/api/v1/items/{id}"), input).await,
        None => post("/api/v1/items", input).await,
    }
}

pub async fn delete_item(id: i64) -> Result<(), ApiError> {
    delete(&format!("/api/v1/items/{id}")).await
}

pub async fn taxes() -> Result<Vec<Tax>, ApiError> {
    get("/api/v1/taxes").await
}

pub async fn save_tax(id: Option<i64>, input: &TaxInput) -> Result<Tax, ApiError> {
    match id {
        Some(id) => post(&format!("/api/v1/taxes/{id}"), input).await,
        None => post("/api/v1/taxes", input).await,
    }
}

pub async fn delete_tax(id: i64) -> Result<(), ApiError> {
    delete(&format!("/api/v1/taxes/{id}")).await
}

pub async fn add_gst_taxes() -> Result<Vec<Tax>, ApiError> {
    post("/api/v1/taxes/gst", &serde_json::json!({})).await
}

pub async fn units() -> Result<Vec<Unit>, ApiError> {
    get("/api/v1/units").await
}

pub async fn add_unit(name: &str, is_whole: bool) -> Result<Unit, ApiError> {
    post(
        "/api/v1/units",
        &serde_json::json!({"name": name, "is_whole": is_whole}),
    )
    .await
}

pub async fn item_groups() -> Result<Vec<ItemGroup>, ApiError> {
    get("/api/v1/item-groups").await
}

pub async fn add_item_group(name: &str, hsn_code: Option<String>) -> Result<ItemGroup, ApiError> {
    post(
        "/api/v1/item-groups",
        &serde_json::json!({"name": name, "hsn_code": hsn_code}),
    )
    .await
}

pub async fn indian_states() -> Result<Vec<String>, ApiError> {
    get("/api/v1/regional/indian-states").await
}

pub async fn countries() -> Result<Vec<String>, ApiError> {
    get("/api/v1/company/countries").await
}

pub async fn country_defaults(country: &str) -> Result<CountryDefaults, ApiError> {
    get(&format!(
        "/api/v1/company/defaults?country={}",
        encode(country)
    ))
    .await
}

pub async fn chart_templates() -> Result<Vec<ChartTemplate>, ApiError> {
    get("/api/v1/accounts/charts").await
}

pub async fn set_up_company(input: &CompanySetup) -> Result<serde_json::Value, ApiError> {
    post("/api/v1/company/setup", input).await
}

pub async fn chart_of_accounts() -> Result<ChartOfAccounts, ApiError> {
    get("/api/v1/accounts").await
}
