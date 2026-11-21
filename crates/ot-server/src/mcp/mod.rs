//! The native MCP server (roadmap step 0.13), served at `/mcp` with Streamable HTTP.
//!
//! It offers the same 25 tools, 3 prompts, 2 guides and Trial Balance widget as the
//! Node connector in `connectors/mcp`, whose catalogue is exported into `catalog.json`
//! by `scripts/export_mcp_catalog.mjs`. Tools run in this process against the book,
//! with no HTTP hop, as the caller found by the API guard: each tool is checked
//! against the same route rule as the REST call it mirrors.
//!
//! Write tools are listed and allowed only when the client sends
//! `x-opentally-mcp-writes: 1`, as the Node connector's `OPENTALLY_MCP_WRITES` did.

pub mod bridge;
pub mod schema;

use std::sync::OnceLock;

use axum::http::{Method, request::Parts};
use ot_core::auth::Perm;
use ot_store::{StoreError, Voucher};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, CompleteRequestParams,
        CompleteResult, GetPromptRequestParams, GetPromptResponse, GetPromptResult, Implementation,
        ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
        PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::{
    auth::{self, Identity},
    db::Database,
};

/// The request header a client sends to enable write tools.
pub const WRITES_HEADER: &str = "x-opentally-mcp-writes";
const WIDGET_MIME: &str = "text/html;profile=mcp-app";
const WIDGET_HTML: &str = include_str!("trial-balance-widget.html");
const MAX_RESULT_BYTES: usize = 768 * 1024;
const INSTRUCTIONS: &str = "OpenTally reads and manages a local INR book. Use dates in YYYY-MM-DD format. Report amounts ending in _paise are exact integer strings. Use the stored voucher ID and revision where required.";

#[derive(Deserialize)]
struct Catalog {
    widget_uri: String,
    tools: Vec<ToolSpec>,
    prompts: Vec<PromptSpec>,
    guides: Vec<GuideSpec>,
}

#[derive(Deserialize)]
struct ToolSpec {
    name: String,
    title: String,
    description: String,
    input_schema: Value,
    write: bool,
    destructive: bool,
    idempotent: bool,
    widget: bool,
}

#[derive(Deserialize)]
struct PromptSpec {
    name: String,
    title: String,
    description: String,
    #[serde(default)]
    arguments: Vec<PromptArgumentSpec>,
    instructions: String,
}

#[derive(Deserialize)]
struct PromptArgumentSpec {
    name: String,
    description: String,
    required: bool,
}

#[derive(Deserialize)]
struct GuideSpec {
    uri: String,
    name: String,
    description: String,
    #[serde(rename = "mimeType")]
    mime_type: String,
    text: String,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("catalog.json")).expect("the MCP catalogue is valid")
    })
}

/// The REST route each tool mirrors, for the permission check.
fn route(tool: &str) -> Option<(Method, &'static str)> {
    use Method as M;
    Some(match tool {
        "get_company" => (M::GET, "/settings"),
        "list_groups" => (M::GET, "/groups"),
        "list_ledgers" => (M::GET, "/ledgers"),
        "list_vouchers" => (M::GET, "/vouchers"),
        "get_voucher" => (M::GET, "/vouchers/{id}"),
        "list_voucher_revisions" => (M::GET, "/vouchers/{id}/changes"),
        "get_voucher_revision" => (M::GET, "/vouchers/{id}/changes/{rev}"),
        "trial_balance" => (M::GET, "/reports/trial-balance"),
        "ledger_statement" => (M::GET, "/reports/ledgers/{id}"),
        "cost_center_totals" => (M::GET, "/reports/cost-centres"),
        "cost_center_postings" => (M::GET, "/reports/cost-centres/postings"),
        "bank_reconciliation" => (M::GET, "/bank/{id}"),
        "preview_import" | "commit_import" => (M::POST, "/import"),
        "create_group" => (M::POST, "/groups"),
        "create_ledger" => (M::POST, "/ledgers"),
        "post_voucher" => (M::POST, "/vouchers"),
        "edit_voucher" | "cancel_voucher" => (M::POST, "/vouchers/{id}/changes"),
        "record_opening" => (M::POST, "/ledgers/{id}/openings"),
        "set_company_name" => (M::POST, "/settings"),
        "close_period" => (M::POST, "/settings/close-period"),
        "import_bank_statement" => (M::POST, "/bank/{id}"),
        "match_bank_rows" => (M::POST, "/bank/{id}/matches"),
        "undo_bank_match" => (M::POST, "/bank/matches/{id}/undo"),
        _ => return None,
    })
}

/// The MCP service. One is made per HTTP request; it holds only a handle to the book.
#[derive(Clone)]
pub struct McpServer {
    db: Database,
}

impl McpServer {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

struct Caller {
    identity: Identity,
    writes: bool,
}

fn caller(context: &RequestContext<RoleServer>) -> Result<Caller, McpError> {
    let parts = context.extensions.get::<Parts>().ok_or_else(|| {
        McpError::invalid_request("This MCP server is served over HTTP only.", None)
    })?;
    let identity = parts
        .extensions
        .get::<Identity>()
        .cloned()
        .ok_or_else(|| McpError::invalid_request("Sign in to OpenTally.", None))?;
    let writes = parts
        .headers
        .get(WRITES_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| matches!(value, "1" | "true"));
    Ok(Caller { identity, writes })
}

fn from_json<T: DeserializeOwned>(value: Value) -> Result<T, McpError> {
    serde_json::from_value(value).map_err(|error| {
        McpError::internal_error(format!("Could not build the MCP reply: {error}"), None)
    })
}

fn tool_value(tool: &ToolSpec, widget_uri: &str) -> Value {
    let mut value = json!({
        "name": tool.name,
        "title": tool.title,
        "description": tool.description,
        "inputSchema": tool.input_schema,
        "annotations": {
            "readOnlyHint": !tool.write,
            "destructiveHint": tool.destructive,
            "idempotentHint": !tool.write || tool.idempotent,
            "openWorldHint": false
        }
    });
    if tool.widget {
        value["_meta"] = json!({
            "ui": {"resourceUri": widget_uri, "visibility": ["model", "app"]},
            "openai/outputTemplate": widget_uri,
            "openai/widgetAccessible": true
        });
    }
    value
}

fn error_result(message: impl Into<String>) -> Result<CallToolResponse, McpError> {
    let result: CallToolResult = from_json(json!({
        "isError": true,
        "content": [{"type": "text", "text": message.into()}]
    }))?;
    Ok(result.into())
}

fn store_message(error: StoreError) -> String {
    match error {
        StoreError::Invalid(message)
        | StoreError::Conflict(message)
        | StoreError::Missing(message) => message,
        StoreError::Database(error) => {
            eprintln!("DuckDB error in an MCP tool: {error}");
            "OpenTally could not complete the operation.".into()
        }
        StoreError::Unavailable => "The book is unavailable.".into(),
    }
}

/// Tool failures that become an `isError` result.
enum ToolFailure {
    Denied,
    Invalid(String),
    Store(StoreError),
}

impl From<StoreError> for ToolFailure {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<auth::AuthError> for ToolFailure {
    fn from(error: auth::AuthError) -> Self {
        match error {
            auth::AuthError::Store(error) => Self::Store(error),
            _ => Self::Denied,
        }
    }
}

/// Takes and removes a field, then decodes the rest of the arguments as a store type.
fn take(args: &mut Map<String, Value>, key: &str) -> Value {
    args.remove(key).unwrap_or(Value::Null)
}

fn id_of(args: &mut Map<String, Value>, key: &str) -> Result<i64, ToolFailure> {
    take(args, key)
        .as_i64()
        .ok_or_else(|| ToolFailure::Invalid(format!("{key}. Required.")))
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, ToolFailure> {
    serde_json::from_value(value).map_err(|error| ToolFailure::Invalid(error.to_string()))
}

fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// One page of each list in a result, with paging details, as the Node connector returns.
fn window(value: Value, keys: &[&str], offset: usize, limit: usize) -> Value {
    let mut data = match value {
        Value::Array(rows) => json!({ "rows": rows }),
        other => other,
    };
    let mut pagination = Map::new();
    for key in keys {
        let rows = data
            .get(*key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total = rows.len();
        let page: Vec<Value> = rows.into_iter().skip(offset).take(limit).collect();
        data[*key] = Value::Array(page);
        pagination.insert(
            (*key).to_owned(),
            json!({
                "offset": offset,
                "limit": limit,
                "total_rows": total,
                "next_offset": if total > offset + limit { json!(offset + limit) } else { Value::Null }
            }),
        );
    }
    data["pagination"] = Value::Object(pagination);
    data
}

impl McpServer {
    async fn run(
        &self,
        name: &str,
        mut args: Map<String, Value>,
        identity: &Identity,
    ) -> Result<Value, ToolFailure> {
        let offset = take(&mut args, "offset").as_u64().unwrap_or(0) as usize;
        let limit = take(&mut args, "limit").as_u64().unwrap_or(25) as usize;
        let rest = Value::Object(args.clone());
        let db = &self.db;
        Ok(match name {
            "get_company" => to_value(db.read(|s| s.settings()).await?),
            "list_groups" => window(
                to_value(db.read(|s| s.groups()).await?),
                &["rows"],
                offset,
                limit,
            ),
            "list_ledgers" => window(
                to_value(db.read(|s| s.ledgers()).await?),
                &["rows"],
                offset,
                limit,
            ),
            "list_vouchers" => {
                let filter = decode(rest)?;
                to_value(db.read(move |s| s.register(filter)).await?)
            }
            "get_voucher" => {
                let id = id_of(&mut args, "voucher_id")?;
                window(
                    to_value(db.read(move |s| s.voucher(id)).await?),
                    &["postings"],
                    offset,
                    limit,
                )
            }
            "list_voucher_revisions" => {
                let id = id_of(&mut args, "voucher_id")?;
                window(
                    to_value(db.read(move |s| s.history(id)).await?),
                    &["rows"],
                    offset,
                    limit,
                )
            }
            "get_voucher_revision" => {
                let id = id_of(&mut args, "voucher_id")?;
                let revision = i32::try_from(id_of(&mut args, "revision")?)
                    .map_err(|_| ToolFailure::Invalid("revision. Too large.".into()))?;
                let event = db.read(move |s| s.history_revision(id, revision)).await?;
                window(to_value(event), &["postings"], offset, limit)
            }
            "trial_balance" => {
                let period = json!({"from": args.get("from"), "to": args.get("to")});
                let report = db
                    .read({
                        let period = decode(period.clone())?;
                        move |s| s.trial_balance(period)
                    })
                    .await?;
                let mut value = window(to_value(report), &["rows"], offset, limit);
                value["period"] = period;
                value
            }
            "ledger_statement" => {
                let id = id_of(&mut args, "ledger_id")?;
                let period = decode(Value::Object(args))?;
                window(
                    to_value(db.read(move |s| s.statement(id, period)).await?),
                    &["rows"],
                    offset,
                    limit,
                )
            }
            "cost_center_totals" => {
                let period = decode(rest)?;
                window(
                    to_value(db.read(move |s| s.cost_totals(period)).await?),
                    &["rows"],
                    offset,
                    limit,
                )
            }
            "cost_center_postings" => {
                let filter = decode(rest)?;
                window(
                    to_value(db.read(move |s| s.cost_postings(filter)).await?),
                    &["rows"],
                    offset,
                    limit,
                )
            }
            "bank_reconciliation" => {
                let id = id_of(&mut args, "ledger_id")?;
                let period = decode(Value::Object(args))?;
                let review = db.read(move |s| s.reconciliation(id, period)).await?;
                window(to_value(review), &["statement", "book"], offset, limit)
            }
            "preview_import" | "commit_import" => {
                let mut body = rest;
                body["commit"] = Value::Bool(name == "commit_import");
                let request = decode(body)?;
                to_value(db.write(move |s| s.import(request)).await?)
            }
            "create_group" => {
                let group = decode(rest)?;
                to_value(db.write(move |s| s.add_group(group)).await?)
            }
            "create_ledger" => {
                let ledger = decode(rest)?;
                to_value(db.write(move |s| s.add_ledger(ledger)).await?)
            }
            "post_voucher" => {
                let voucher: ot_store::NewVoucher = decode(take(&mut args, "voucher"))?;
                identity.require(auth::voucher_doctype(voucher.kind_key()), Perm::Submit)?;
                let actor = identity.actor();
                to_value(
                    db.write(move |s| s.add_voucher_by(voucher, &actor))
                        .await?
                        .0,
                )
            }
            "edit_voucher" | "cancel_voucher" => {
                let id = id_of(&mut args, "voucher_id")?;
                let mut change: ot_store::VoucherChange = decode(Value::Object(args))?;
                let current: Voucher = db.read(move |s| s.voucher(id)).await?;
                match change.replacement() {
                    Some(replacement) => {
                        identity.require(auth::voucher_doctype(current.kind()), Perm::Amend)?;
                        identity
                            .require(auth::voucher_doctype(replacement.kind_key()), Perm::Amend)?;
                    }
                    None => {
                        identity.require(auth::voucher_doctype(current.kind()), Perm::Cancel)?
                    }
                }
                if identity.user.is_some() {
                    change.set_actor(identity.actor());
                }
                to_value(db.write(move |s| s.change_voucher(id, change)).await?)
            }
            "record_opening" => {
                let id = id_of(&mut args, "ledger_id")?;
                let opening = decode(Value::Object(args))?;
                db.write(move |s| s.record_opening(id, opening)).await?;
                json!({"saved": true})
            }
            "set_company_name" => {
                let company = decode(rest)?;
                to_value(db.write(move |s| s.name_company(company)).await?)
            }
            "close_period" => {
                let request = decode(rest)?;
                to_value(db.write(move |s| s.close_period(request)).await?)
            }
            "import_bank_statement" => {
                let id = id_of(&mut args, "ledger_id")?;
                let statement = decode(Value::Object(args))?;
                to_value(db.write(move |s| s.import_statement(id, statement)).await?)
            }
            "match_bank_rows" => {
                let id = id_of(&mut args, "ledger_id")?;
                let request = decode(Value::Object(args))?;
                to_value(db.write(move |s| s.reconcile(id, request)).await?)
            }
            "undo_bank_match" => {
                let id = id_of(&mut args, "match_id")?;
                let request = decode(Value::Object(args))?;
                db.write(move |s| s.undo_reconciliation(id, request))
                    .await?;
                json!({"saved": true})
            }
            _ => return Err(ToolFailure::Invalid("Unknown tool.".into())),
        })
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        let capabilities: ServerCapabilities = serde_json::from_value(json!({
            "tools": {}, "resources": {}, "prompts": {}, "completions": {}
        }))
        .unwrap_or_default();
        let mut info = ServerConfig::new(capabilities);
        let mut server = Implementation::new("opentally", env!("CARGO_PKG_VERSION"));
        server.title = Some("OpenTally".into());
        info.server_info = server;
        info.instructions = Some(INSTRUCTIONS.into());
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let caller = caller(&context)?;
        let catalog = catalog();
        let tools: Vec<Value> = catalog
            .tools
            .iter()
            .filter(|tool| caller.writes || !tool.write)
            .map(|tool| tool_value(tool, &catalog.widget_uri))
            .collect();
        from_json(json!({ "tools": tools }))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let caller = caller(&context)?;
        let Some(tool) = catalog()
            .tools
            .iter()
            .find(|tool| tool.name == request.name && (caller.writes || !tool.write))
        else {
            return error_result("This OpenTally tool is disabled or unavailable.");
        };
        let args = Value::Object(request.arguments.unwrap_or_default());
        let issues = schema::validate(&tool.input_schema, &args);
        if !issues.is_empty() {
            let text: String = format!("Invalid tool arguments. {}", issues.join(" "))
                .chars()
                .take(1200)
                .collect();
            return error_result(text);
        }
        let (method, path) = route(&tool.name).expect("every catalogue tool has a route");
        if !auth::route_allowed(&caller.identity, &method, path) {
            return error_result("Your role does not allow this action.");
        }
        let Value::Object(args) = args else {
            return error_result("Invalid tool arguments.");
        };
        match self.run(&tool.name, args, &caller.identity).await {
            Ok(data) => {
                let text = data.to_string();
                if text.len() > MAX_RESULT_BYTES {
                    if !tool.write {
                        return error_result(
                            "The result is too large. Narrow the dates or request fewer rows.",
                        );
                    }
                    let summary =
                        json!({"saved": true, "id": data.get("id"), "code": data.get("code")});
                    let result: CallToolResult = from_json(json!({
                        "content": [{"type": "text", "text": "OpenTally accepted the change. Its full result is too large to return. Review the saved records in OpenTally."}],
                        "structuredContent": summary
                    }))?;
                    return Ok(result.into());
                }
                let result: CallToolResult = from_json(json!({
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": data
                }))?;
                Ok(result.into())
            }
            Err(ToolFailure::Denied) => error_result("Your role does not allow this action."),
            Err(ToolFailure::Invalid(message)) => {
                error_result(format!("Invalid tool arguments. {message}"))
            }
            Err(ToolFailure::Store(error)) => error_result(store_message(error)),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        caller(&context)?;
        let catalog = catalog();
        let mut resources: Vec<Value> = catalog
            .guides
            .iter()
            .map(|g| json!({"uri": g.uri, "name": g.name, "description": g.description, "mimeType": g.mime_type}))
            .collect();
        resources.push(json!({
            "uri": catalog.widget_uri,
            "name": "OpenTally Trial Balance",
            "description": "View accounting and ledger balances in a paginated table.",
            "mimeType": WIDGET_MIME
        }));
        from_json(json!({ "resources": resources }))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        from_json(json!({ "resourceTemplates": [] }))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        caller(&context)?;
        let catalog = catalog();
        let contents = if request.uri == catalog.widget_uri {
            json!([{
                "uri": request.uri, "mimeType": WIDGET_MIME, "text": WIDGET_HTML,
                "_meta": {"ui": {"prefersBorder": true, "csp": {"connectDomains": [], "resourceDomains": []}}}
            }])
        } else if let Some(guide) = catalog.guides.iter().find(|g| g.uri == request.uri) {
            json!([{"uri": guide.uri, "mimeType": guide.mime_type, "text": guide.text}])
        } else {
            return Err(McpError::resource_not_found("Resource not found.", None));
        };
        let result: ReadResourceResult = from_json(json!({ "contents": contents }))?;
        Ok(result.into())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        caller(&context)?;
        let prompts: Vec<Value> = catalog()
            .prompts
            .iter()
            .map(|p| {
                let mut prompt = json!({"name": p.name, "title": p.title, "description": p.description});
                if !p.arguments.is_empty() {
                    prompt["arguments"] = p
                        .arguments
                        .iter()
                        .map(|a| json!({"name": a.name, "description": a.description, "required": a.required}))
                        .collect();
                }
                prompt
            })
            .collect();
        from_json(json!({ "prompts": prompts }))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        caller(&context)?;
        let prompt = catalog()
            .prompts
            .iter()
            .find(|p| p.name == request.name)
            .ok_or_else(|| McpError::invalid_params("Unknown OpenTally prompt.", None))?;
        let args = request.arguments.unwrap_or_default();
        let mut lines = vec![prompt.instructions.clone()];
        for argument in &prompt.arguments {
            let value = args
                .get(&argument.name)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let valid = if argument.name == "ledger_id" {
                !value.is_empty()
                    && !value.starts_with('0')
                    && value.bytes().all(|b| b.is_ascii_digit())
            } else {
                is_date(value)
            };
            if !valid {
                return Err(McpError::invalid_params(
                    format!("Supply a valid {}.", argument.name),
                    None,
                ));
            }
            lines.push(format!("{} = {value}", argument.name));
        }
        let result: GetPromptResult = from_json(json!({
            "description": prompt.description,
            "messages": [{"role": "user", "content": {"type": "text", "text": lines.join("\n")}}]
        }))?;
        Ok(result.into())
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        caller(&context)?;
        let reference = to_value(&request.r#ref);
        let argument = request.argument.name.as_str();
        let is_prompt_date = reference["type"] == "ref/prompt"
            && matches!(argument, "from" | "to")
            && catalog().prompts.iter().any(|p| {
                Some(p.name.as_str()) == reference["name"].as_str()
                    && p.arguments.iter().any(|a| a.name == argument)
            });
        let values = if is_prompt_date {
            complete_date(argument, &request.argument.value, today())
        } else {
            Vec::new()
        };
        from_json(
            json!({"completion": {"values": values, "total": values.len(), "hasMore": false}}),
        )
    }
}

fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

/// Today's date (UTC) as (year, month).
fn today() -> (i32, u32) {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400) as i64;
    let (year, month, _) = civil_from_days(days);
    (year, month)
}

/// Days since 1970-01-01 to (year, month, day), from Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    (year, month, day)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 31,
    }
}

/// Date suggestions, as the Node connector gives: the first day of this and the two
/// previous months for `from`, the last day of this and the two previous months for `to`.
fn complete_date(field: &str, prefix: &str, (year, month): (i32, u32)) -> Vec<String> {
    (0..3)
        .map(|back| {
            let index = year * 12 + month as i32 - 1 - back;
            let (y, m) = (index.div_euclid(12), index.rem_euclid(12) as u32 + 1);
            let day = if field == "from" {
                1
            } else {
                days_in_month(y, m)
            };
            format!("{y:04}-{m:02}-{day:02}")
        })
        .filter(|date| date.starts_with(prefix))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_matches_the_node_connector() {
        let catalog = catalog();
        assert_eq!(catalog.tools.len(), 25);
        assert_eq!(catalog.tools.iter().filter(|t| !t.write).count(), 13);
        assert_eq!(catalog.prompts.len(), 3);
        assert_eq!(catalog.guides.len(), 2);
        for tool in &catalog.tools {
            let (method, path) =
                route(&tool.name).unwrap_or_else(|| panic!("no route for {}", tool.name));
            // Every write tool mirrors a POST; preview_import also posts, but rolls back.
            assert!(!tool.write || method == Method::POST, "{}", tool.name);
            assert!(auth::route_allowed(
                &Identity {
                    user: None,
                    grant: ot_core::auth::Grant::new(
                        vec![ot_core::auth::Role::SystemManager],
                        None
                    )
                },
                &method,
                path
            ));
        }
        assert!(WIDGET_HTML.contains("OpenTally Trial Balance"));
    }

    #[test]
    fn windows_lists_like_the_node_connector() {
        let value = window(json!({"rows": [1, 2, 3], "total": 6}), &["rows"], 1, 1);
        assert_eq!(value["rows"], json!([2]));
        assert_eq!(value["total"], 6);
        assert_eq!(
            value["pagination"]["rows"],
            json!({"offset": 1, "limit": 1, "total_rows": 3, "next_offset": 2})
        );
        let last = window(json!([1, 2]), &["rows"], 0, 25);
        assert_eq!(last["pagination"]["rows"]["next_offset"], Value::Null);
    }

    #[test]
    fn dates_complete_to_month_starts_and_ends() {
        assert_eq!(
            complete_date("from", "", (2026, 1)),
            vec!["2026-01-01", "2025-12-01", "2025-11-01"]
        );
        assert_eq!(
            complete_date("to", "", (2024, 3)),
            vec!["2024-03-31", "2024-02-29", "2024-01-31"]
        );
        assert_eq!(
            complete_date("from", "2025", (2026, 1)),
            vec!["2025-12-01", "2025-11-01"]
        );
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_819), (2027, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert!(is_date("2025-09-01") && !is_date("2025-9-1"));
    }
}
