//! HTTP tests of sign-in, sessions, roles and API tokens against an in-memory book.

use std::net::SocketAddr;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::connect_info::MockConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use ot_store::Store;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::{auth::AppState, db::Database, router};

const LOCAL: &str = "127.0.0.1:40000";
const REMOTE: &str = "192.168.1.20:40000";

struct App {
    state: AppState,
}

struct Reply {
    status: StatusCode,
    cookies: Vec<String>,
    body: Value,
}

impl App {
    fn new() -> Self {
        let db = Database::start(Store::open_in_memory().unwrap()).unwrap();
        Self {
            state: AppState::new(db, false),
        }
    }

    fn app(&self, peer: &str) -> Router {
        router(self.state.clone()).layer(MockConnectInfo(peer.parse::<SocketAddr>().unwrap()))
    }

    async fn send(
        &self,
        peer: &str,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> Reply {
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = self.app(peer).oneshot(request).await.unwrap();
        let status = response.status();
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply {
            status,
            cookies,
            body,
        }
    }

    async fn get(&self, uri: &str, headers: &[(&str, &str)]) -> Reply {
        self.send(LOCAL, Method::GET, uri, headers, None).await
    }

    async fn post(&self, uri: &str, headers: &[(&str, &str)], body: Value) -> Reply {
        self.send(LOCAL, Method::POST, uri, headers, Some(body))
            .await
    }

    async fn delete(&self, uri: &str, headers: &[(&str, &str)]) -> Reply {
        self.send(LOCAL, Method::DELETE, uri, headers, None).await
    }
}

/// A signed-in browser: its cookie header and CSRF token.
struct Browser {
    cookie: String,
    csrf: String,
}

impl Browser {
    fn from(reply: &Reply) -> Self {
        let value = |name: &str| {
            reply
                .cookies
                .iter()
                .find_map(|c| c.strip_prefix(&format!("{name}=")))
                .and_then(|c| c.split(';').next())
                .unwrap()
                .to_owned()
        };
        let (session, csrf) = (value("ot_session"), value("ot_csrf"));
        Self {
            cookie: format!("ot_session={session}; ot_csrf={csrf}"),
            csrf,
        }
    }

    fn read(&self) -> Vec<(&str, &str)> {
        vec![("cookie", self.cookie.as_str())]
    }

    fn write(&self) -> Vec<(&str, &str)> {
        vec![
            ("cookie", self.cookie.as_str()),
            ("x-csrf-token", self.csrf.as_str()),
        ]
    }
}

fn payment(n: u32, other_ledger: i64) -> Value {
    json!({"kind": "payment", "request_id": format!("29daeb83-d7be-4576-aca0-{n:012}"),
           "date": "2025-09-03", "amount": "250.00", "cash_ledger_id": 1,
           "other_ledger_id": other_ledger, "reference": "R", "narration": "Tea"})
}

fn journal(n: u32) -> Value {
    json!({"kind": "journal", "request_id": format!("29daeb83-d7be-4576-aca0-{n:012}"),
           "date": "2025-09-03", "postings": [
               {"ledger_id": 1, "side": "debit", "amount": "10.00"},
               {"ledger_id": 2, "side": "credit", "amount": "10.00"}]})
}

async fn set_up_admin(app: &App) -> Browser {
    let reply = app
        .post(
            "/api/v1/setup",
            &[],
            json!({"username": "asha", "full_name": "Asha Patel", "password": "correct horse battery"}),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(reply.cookies.iter().any(|c| c.contains("HttpOnly")));
    Browser::from(&reply)
}

#[tokio::test]
async fn a_book_without_users_is_open_only_to_this_computer() {
    let app = App::new();
    assert_eq!(app.get("/api/ledgers", &[]).await.status, StatusCode::OK);
    assert_eq!(app.get("/api/v1/ledgers", &[]).await.status, StatusCode::OK);
    let remote = app
        .send(REMOTE, Method::GET, "/api/ledgers", &[], None)
        .await;
    assert_eq!(remote.status, StatusCode::UNAUTHORIZED);
    let proxied = app
        .get("/api/ledgers", &[("x-forwarded-for", "203.0.113.9")])
        .await;
    assert_eq!(proxied.status, StatusCode::UNAUTHORIZED);
    let state = app.get("/api/v1/session", &[]).await;
    assert_eq!(state.body["setup_required"], true);
    // The first user can be created only from this computer.
    let remote_setup = app
        .send(
            REMOTE,
            Method::POST,
            "/api/v1/setup",
            &[],
            Some(json!({"username": "x", "full_name": "X", "password": "correct horse battery"})),
        )
        .await;
    assert_eq!(remote_setup.status, StatusCode::FORBIDDEN);
    // Unknown API routes are refused, not passed through.
    assert_eq!(
        app.get("/api/nothing", &[]).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn after_setup_every_request_needs_a_session_and_changes_need_csrf() {
    let app = App::new();
    let asha = set_up_admin(&app).await;
    assert_eq!(
        app.get("/api/ledgers", &[]).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.get("/api/ledgers", &asha.read()).await.status,
        StatusCode::OK
    );

    let ledger = json!({"name": "Office Expenses", "kind": "general"});
    let no_csrf = app.post("/api/ledgers", &asha.read(), ledger.clone()).await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);
    let created = app.post("/api/ledgers", &asha.write(), ledger).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);

    let again = app
        .post(
            "/api/v1/setup",
            &[],
            json!({"username": "b", "full_name": "B", "password": "correct horse battery"}),
        )
        .await;
    assert_eq!(again.status, StatusCode::CONFLICT);

    let state = app.get("/api/v1/session", &asha.read()).await;
    assert_eq!(state.body["user"]["username"], "asha");
    assert_eq!(state.body["csrf_token"], asha.csrf.as_str());

    let out = app
        .send(LOCAL, Method::DELETE, "/api/v1/session", &asha.read(), None)
        .await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.get("/api/ledgers", &asha.read()).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn roles_follow_frappe_books_permissions() {
    let app = App::new();
    let asha = set_up_admin(&app).await;
    let expense = app
        .post(
            "/api/ledgers",
            &asha.write(),
            json!({"name": "Tea", "kind": "general"}),
        )
        .await
        .body["id"]
        .as_i64()
        .unwrap();
    let added = app
        .post(
            "/api/v1/users",
            &asha.write(),
            json!({"username": "ravi", "full_name": "Ravi Shah",
                   "password": "another long secret", "roles": ["books_user"]}),
        )
        .await;
    assert_eq!(added.status, StatusCode::CREATED, "{}", added.body);

    let signed_in = app
        .post(
            "/api/v1/session",
            &[],
            json!({"username": "ravi", "password": "another long secret"}),
        )
        .await;
    assert_eq!(signed_in.status, StatusCode::OK);
    let ravi = Browser::from(&signed_in);

    // A Books User takes payments and sees the ledgers to pick from.
    assert_eq!(
        app.get("/api/ledgers", &ravi.read()).await.status,
        StatusCode::OK
    );
    let paid = app
        .post("/api/vouchers", &ravi.write(), payment(1, expense))
        .await;
    assert_eq!(paid.status, StatusCode::CREATED, "{}", paid.body);
    let id = paid.body["id"].as_i64().unwrap();
    let history = app
        .get(&format!("/api/vouchers/{id}/changes"), &ravi.read())
        .await;
    assert_eq!(history.body[0]["actor"], "Ravi Shah (ravi)");

    // ...but not journals, accounts, reports or users.
    let journal_reply = app.post("/api/vouchers", &ravi.write(), journal(2)).await;
    assert_eq!(journal_reply.status, StatusCode::FORBIDDEN);
    let ledger = app
        .post(
            "/api/ledgers",
            &ravi.write(),
            json!({"name": "X", "kind": "general"}),
        )
        .await;
    assert_eq!(ledger.status, StatusCode::FORBIDDEN);
    let report = app
        .get(
            "/api/reports/trial-balance?from=2025-09-01&to=2025-09-30",
            &ravi.read(),
        )
        .await;
    assert_eq!(report.status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.get("/api/v1/users", &ravi.read()).await.status,
        StatusCode::FORBIDDEN
    );

    // A manager may post the journal.
    let manager_journal = app.post("/api/vouchers", &asha.write(), journal(3)).await;
    assert_eq!(
        manager_journal.status,
        StatusCode::CREATED,
        "{}",
        manager_journal.body
    );
}

#[tokio::test]
async fn api_tokens_act_for_their_user_within_their_scope() {
    let app = App::new();
    let asha = set_up_admin(&app).await;
    let made = app
        .post(
            "/api/v1/tokens",
            &asha.write(),
            json!({"name": "Claude", "scope": "read"}),
        )
        .await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let secret = made.body["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("ot_"));
    let bearer = format!("Bearer {secret}");
    let auth = [("authorization", bearer.as_str())];

    assert_eq!(app.get("/api/ledgers", &auth).await.status, StatusCode::OK);
    let report = app
        .get(
            "/api/reports/trial-balance?from=2025-09-01&to=2025-09-30",
            &auth,
        )
        .await;
    assert_eq!(report.status, StatusCode::OK);
    // A read token cannot post, needs no CSRF header, and cannot mint stronger tokens.
    let post = app.post("/api/vouchers", &auth, journal(4)).await;
    assert_eq!(post.status, StatusCode::FORBIDDEN);
    let stronger = app
        .post(
            "/api/v1/tokens",
            &auth,
            json!({"name": "More", "scope": "submit"}),
        )
        .await;
    assert_eq!(stronger.status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.get("/api/v1/users", &auth).await.status,
        StatusCode::FORBIDDEN
    );

    let id = made.body["id"].as_i64().unwrap();
    let revoked = app
        .send(
            LOCAL,
            Method::DELETE,
            &format!("/api/v1/tokens/{id}"),
            &asha.write(),
            None,
        )
        .await;
    assert_eq!(revoked.body["revoked"], true);
    assert_eq!(
        app.get("/api/ledgers", &auth).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn repeated_wrong_passwords_are_throttled() {
    let app = App::new();
    set_up_admin(&app).await;
    for _ in 0..5 {
        let reply = app
            .post(
                "/api/v1/session",
                &[],
                json!({"username": "asha", "password": "wrong password"}),
            )
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }
    let blocked = app
        .post(
            "/api/v1/session",
            &[],
            json!({"username": "asha", "password": "correct horse battery"}),
        )
        .await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn responses_match_the_shared_api_types() {
    let app = App::new();
    let setup = app
        .post(
            "/api/v1/setup",
            &[],
            json!({"username": "asha", "full_name": "Asha Patel", "password": "correct horse battery"}),
        )
        .await;
    let state: ot_api_types::SessionState = serde_json::from_value(setup.body.clone()).unwrap();
    assert!(state.signed_in && state.csrf_token.is_some());
    assert_eq!(state.user.unwrap().roles, vec!["system_manager"]);
    let asha = Browser::from(&setup);

    let expense = app
        .post(
            "/api/v1/ledgers",
            &asha.write(),
            json!({"name": "Tea", "kind": "general"}),
        )
        .await
        .body["id"]
        .as_i64()
        .unwrap();
    app.post("/api/v1/vouchers", &asha.write(), payment(9, expense))
        .await;
    let filter = ot_api_types::DayBookFilter {
        kind: Some("payment".into()),
        page: Some(1),
        ..Default::default()
    };
    let reply = app
        .get(
            &format!("/api/v1/vouchers?{}", filter.query()),
            &asha.read(),
        )
        .await;
    let book: ot_api_types::DayBook = serde_json::from_value(reply.body).unwrap();
    assert_eq!(book.count, 1);
    assert_eq!(book.rows[0].other_account, "Tea");
    assert_eq!(
        ot_api_types::format_paise(&book.rows[0].amount_paise),
        "250.00"
    );

    let denied = app.get("/api/v1/users", &[]).await;
    let error: ot_api_types::ApiErrorBody = serde_json::from_value(denied.body).unwrap();
    assert_eq!(error.error, "Sign in to OpenTally.");
}

#[tokio::test]
async fn the_new_interface_is_served_at_app() {
    let app = App::new();
    // Tests run from the crate directory, where no UI has been built.
    let page = app.get("/app/", &[]).await;
    assert_eq!(page.status, StatusCode::OK);
    let built = std::env::temp_dir().join(format!("ot-ui-{}", std::process::id()));
    std::fs::create_dir_all(&built).unwrap();
    std::fs::write(built.join("index.html"), "<p>new ui</p>").unwrap();
    std::fs::write(built.join("app.js"), "1").unwrap();
    let router = crate::router_with_ui(app.state.clone(), built.clone())
        .layer(MockConnectInfo(LOCAL.parse::<SocketAddr>().unwrap()));
    for (uri, expected) in [
        ("/app/", StatusCode::OK),
        ("/app/app.js", StatusCode::OK),
        ("/app/day-book", StatusCode::OK),
        ("/app", StatusCode::PERMANENT_REDIRECT),
    ] {
        let response = router
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{uri}");
    }
    // The classic screens stay at the root.
    let root = router
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(root.status(), StatusCode::OK);
    let _ = std::fs::remove_dir_all(built);
}

/// One JSON-RPC call to `/mcp`, as an MCP client sends it.
async fn mcp(
    app: &App,
    peer: &str,
    headers: &[(&str, &str)],
    method: &str,
    params: Value,
) -> Reply {
    let mut all = vec![
        ("host", "127.0.0.1:3000"),
        ("accept", "application/json, text/event-stream"),
        ("mcp-protocol-version", "2025-06-18"),
    ];
    all.extend_from_slice(headers);
    app.send(
        peer,
        Method::POST,
        "/mcp",
        &all,
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})),
    )
    .await
}

#[tokio::test]
async fn mcp_offers_the_node_connector_catalogue() {
    let app = App::new();
    let init = mcp(&app, LOCAL, &[], "initialize", json!({
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}
    }))
    .await;
    assert_eq!(init.status, StatusCode::OK, "{}", init.body);
    for capability in ["tools", "resources", "prompts", "completions"] {
        assert!(
            init.body["result"]["capabilities"][capability].is_object(),
            "{capability}"
        );
    }

    let read_only = mcp(&app, LOCAL, &[], "tools/list", json!({})).await;
    let tools = read_only.body["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 13);
    assert!(
        tools
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true)
    );
    let trial = tools.iter().find(|t| t["name"] == "trial_balance").unwrap();
    assert_eq!(
        trial["_meta"]["ui"]["resourceUri"],
        "ui://opentally/trial-balance-v1.html"
    );
    let all = mcp(
        &app,
        LOCAL,
        &[("x-opentally-mcp-writes", "1")],
        "tools/list",
        json!({}),
    )
    .await;
    assert_eq!(all.body["result"]["tools"].as_array().unwrap().len(), 25);

    // A write tool is unavailable without the writes header; bad arguments are refused.
    let closed = mcp(
        &app,
        LOCAL,
        &[],
        "tools/call",
        json!({"name": "close_period",
        "arguments": {"through": "2025-09-30", "actor": "T", "reason": "T"}}),
    )
    .await;
    assert_eq!(closed.body["result"]["isError"], true);
    let invalid = mcp(
        &app,
        LOCAL,
        &[],
        "tools/call",
        json!({"name": "trial_balance",
        "arguments": {"from": "invalid", "to": "2025-09-30"}}),
    )
    .await;
    assert_eq!(invalid.body["result"]["isError"], true);
    assert!(
        invalid.body["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("from. Invalid format.")
    );
    let company = mcp(
        &app,
        LOCAL,
        &[],
        "tools/call",
        json!({"name": "get_company", "arguments": {}}),
    )
    .await;
    assert_eq!(
        company.body["result"]["structuredContent"]["company_name"],
        "Local book"
    );

    let resources = mcp(&app, LOCAL, &[], "resources/list", json!({})).await;
    assert_eq!(
        resources.body["result"]["resources"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let guide = mcp(
        &app,
        LOCAL,
        &[],
        "resources/read",
        json!({"uri": "opentally://guide/accounting"}),
    )
    .await;
    assert!(
        guide.body["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("INR company")
    );
    let widget = mcp(
        &app,
        LOCAL,
        &[],
        "resources/read",
        json!({"uri": "ui://opentally/trial-balance-v1.html"}),
    )
    .await;
    assert_eq!(
        widget.body["result"]["contents"][0]["mimeType"],
        "text/html;profile=mcp-app"
    );

    let prompts = mcp(&app, LOCAL, &[], "prompts/list", json!({})).await;
    assert_eq!(
        prompts.body["result"]["prompts"].as_array().unwrap().len(),
        3
    );
    let prompt = mcp(
        &app,
        LOCAL,
        &[],
        "prompts/get",
        json!({"name": "review_period",
        "arguments": {"from": "2025-09-01", "to": "2025-09-30"}}),
    )
    .await;
    assert!(
        prompt.body["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("from = 2025-09-01")
    );
    let bad_prompt = mcp(
        &app,
        LOCAL,
        &[],
        "prompts/get",
        json!({"name": "review_period",
        "arguments": {"from": "x", "to": "2025-09-30"}}),
    )
    .await;
    assert!(bad_prompt.body["error"].is_object());
    let completion = mcp(&app, LOCAL, &[], "completion/complete", json!({
        "ref": {"type": "ref/prompt", "name": "review_period"}, "argument": {"name": "from", "value": ""}})).await;
    let first = completion.body["result"]["completion"]["values"][0]
        .as_str()
        .unwrap();
    assert!(first.len() == 10 && first.ends_with("-01"), "{first}");
}

#[tokio::test]
async fn mcp_uses_the_callers_rights() {
    let app = App::new();
    // Without users only this computer may connect, and only by an allowed host name.
    let remote = mcp(&app, REMOTE, &[], "tools/list", json!({})).await;
    assert_eq!(remote.status, StatusCode::UNAUTHORIZED);
    let rebinding = app
        .send(
            LOCAL,
            Method::POST,
            "/mcp",
            &[
                ("host", "attacker.example"),
                ("accept", "application/json, text/event-stream"),
            ],
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})),
        )
        .await;
    assert_ne!(rebinding.status, StatusCode::OK);

    let asha = set_up_admin(&app).await;
    assert_eq!(
        mcp(&app, LOCAL, &[], "tools/list", json!({})).await.status,
        StatusCode::UNAUTHORIZED
    );
    let made = app
        .post(
            "/api/v1/tokens",
            &asha.write(),
            json!({"name": "Claude", "scope": "read"}),
        )
        .await;
    let bearer = format!("Bearer {}", made.body["secret"].as_str().unwrap());
    let auth = [
        ("authorization", bearer.as_str()),
        ("x-opentally-mcp-writes", "1"),
    ];
    let read = mcp(
        &app,
        LOCAL,
        &auth,
        "tools/call",
        json!({"name": "list_ledgers", "arguments": {}}),
    )
    .await;
    assert_eq!(
        read.body["result"]["structuredContent"]["rows"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // A read token may see the write tools but not use them.
    let write = mcp(
        &app,
        LOCAL,
        &auth,
        "tools/call",
        json!({"name": "set_company_name", "arguments": {"name": "X"}}),
    )
    .await;
    assert_eq!(write.body["result"]["isError"], true);
    assert_eq!(
        write.body["result"]["content"][0]["text"],
        "Your role does not allow this action."
    );
}

#[tokio::test]
async fn chart_of_accounts_and_settings_follow_frappe_roles() {
    let app = App::new();
    let asha = set_up_admin(&app).await;
    let charts = app.get("/api/v1/accounts/charts", &asha.read()).await;
    assert_eq!(charts.body.as_array().unwrap().len(), 14);
    let applied = app
        .post(
            "/api/v1/accounts/chart",
            &asha.write(),
            json!({"chart": "Tally (India) - Reserved Groups"}),
        )
        .await;
    assert_eq!(applied.status, StatusCode::OK, "{}", applied.body);
    assert_eq!(applied.body["groups_created"], 28);
    let chart = app.get("/api/v1/accounts", &asha.read()).await;
    assert_eq!(chart.body["unclassified_ledgers"], 0);
    let unknown = app
        .post(
            "/api/v1/accounts/chart",
            &asha.write(),
            json!({"chart": "Atlantis"}),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);

    for (username, role) in [("ravi", "books_user"), ("veena", "viewer")] {
        app.post(
            "/api/v1/users",
            &asha.write(),
            json!({"username": username, "full_name": username, "password": "another long secret", "roles": [role]}),
        )
        .await;
    }
    let sign_in = |username: &'static str| {
        let app = &app;
        async move {
            Browser::from(
                &app.post(
                    "/api/v1/session",
                    &[],
                    json!({"username": username, "password": "another long secret"}),
                )
                .await,
            )
        }
    };
    let ravi = sign_in("ravi").await;
    let veena = sign_in("veena").await;
    // Books Users read the accounting settings but not the chart, and change neither.
    assert_eq!(
        app.get("/api/v1/settings/accounting", &ravi.read())
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        app.get("/api/v1/accounts", &ravi.read()).await.status,
        StatusCode::FORBIDDEN
    );
    let change = app
        .post(
            "/api/v1/settings/accounting",
            &ravi.write(),
            json!({"country": "India"}),
        )
        .await;
    assert_eq!(change.status, StatusCode::FORBIDDEN);
    // Viewers read the chart and change nothing.
    assert_eq!(
        app.get("/api/v1/accounts", &veena.read()).await.status,
        StatusCode::OK
    );
    let root = app
        .post(
            "/api/v1/groups/1/root-type",
            &veena.write(),
            json!({"root_type": "Asset"}),
        )
        .await;
    assert_eq!(root.status, StatusCode::FORBIDDEN);

    let saved = app
        .post(
            "/api/v1/settings/accounting",
            &asha.write(),
            json!({"country": "India", "fiscal_year_start": "2025-04-01", "setup_complete": true}),
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    assert_eq!(saved.body["fiscal_year_start"], "2025-04-01");
    assert!(saved.body["round_off_ledger_id"].is_i64());
}

#[tokio::test]
async fn company_setup_wizard_runs_once_for_a_system_manager() {
    let app = App::new();
    let asha = set_up_admin(&app).await;
    let state: ot_api_types::SessionState =
        serde_json::from_value(app.get("/api/v1/session", &asha.read()).await.body).unwrap();
    assert!(!state.company_setup_complete);
    let countries = app.get("/api/v1/company/countries", &asha.read()).await;
    assert!(
        countries
            .body
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "India")
    );
    let defaults: ot_api_types::CountryDefaults = serde_json::from_value(
        app.get("/api/v1/company/defaults?country=India", &asha.read())
            .await
            .body,
    )
    .unwrap();
    assert_eq!(defaults.currency.as_deref(), Some("INR"));
    assert!(defaults.fiscal_year_start.ends_with("-04-01"));

    app.post(
        "/api/v1/users",
        &asha.write(),
        json!({"username": "mira", "full_name": "Mira", "password": "another long secret", "roles": ["books_manager"]}),
    )
    .await;
    let mira = Browser::from(
        &app.post(
            "/api/v1/session",
            &[],
            json!({"username": "mira", "password": "another long secret"}),
        )
        .await,
    );
    let setup =
        json!({"company_name": "Shree Traders", "country": "India", "bank_name": "HDFC Bank"});
    let denied = app
        .post("/api/v1/company/setup", &mira.write(), setup.clone())
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    let done = app
        .post("/api/v1/company/setup", &asha.write(), setup.clone())
        .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(done.body["settings"]["setup_complete"], true);
    let state: ot_api_types::SessionState =
        serde_json::from_value(app.get("/api/v1/session", &asha.read()).await.body).unwrap();
    assert!(state.company_setup_complete);
    let again = app
        .post("/api/v1/company/setup", &asha.write(), setup)
        .await;
    assert_eq!(again.status, StatusCode::CONFLICT);

    let chart: ot_api_types::ChartOfAccounts =
        serde_json::from_value(app.get("/api/v1/accounts", &mira.read()).await.body).unwrap();
    assert_eq!(chart.unclassified_ledgers, 0);
    let bank = chart
        .ledgers
        .iter()
        .find(|l| l.name == "HDFC Bank")
        .unwrap();
    assert_eq!(bank.effective_type.as_deref(), Some("Bank"));
    assert_eq!(bank.root_type.as_deref(), Some("Asset"));
    let templates: Vec<ot_api_types::ChartTemplate> =
        serde_json::from_value(app.get("/api/v1/accounts/charts", &mira.read()).await.body)
            .unwrap();
    assert_eq!(templates.len(), 14);
    let book = app.get("/api/v1/settings", &mira.read()).await;
    assert_eq!(book.body["company_name"], "Shree Traders");
}

#[tokio::test]
async fn number_series_follow_frappe_roles() {
    let app = App::new();
    let asha = set_up_admin(&app).await;
    let types = app.get("/api/v1/series/types", &asha.read()).await;
    assert_eq!(types.body.as_array().unwrap().len(), 9);
    assert_eq!(types.body[0]["key"], "journal_entry");

    let list: Vec<ot_api_types::NumberSeries> =
        serde_json::from_value(app.get("/api/v1/series", &asha.read()).await.body).unwrap();
    let sinv = list.iter().find(|s| s.prefix == "SINV-").unwrap();
    assert!(sinv.is_default);
    assert_eq!(sinv.next_name, "SINV-1001");
    assert_eq!(sinv.reference_label, "Sales Invoice");

    let added = app
        .post(
            "/api/v1/series",
            &asha.write(),
            json!({"prefix": "GST 26-", "reference_type": "sales_invoice", "start": 1, "pad_zeros": 3}),
        )
        .await;
    assert_eq!(added.status, StatusCode::CREATED, "{}", added.body);
    assert_eq!(added.body["next_name"], "GST 26-001");
    let again = app
        .post(
            "/api/v1/series",
            &asha.write(),
            json!({"prefix": "GST 26-", "reference_type": "sales_invoice"}),
        )
        .await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    let bad = app
        .post(
            "/api/v1/series",
            &asha.write(),
            json!({"prefix": "A?B", "reference_type": "payment"}),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    // The prefix travels percent-encoded in the path.
    let default = app
        .post(
            "/api/v1/series/GST%2026-",
            &asha.write(),
            json!({"make_default": true}),
        )
        .await;
    assert_eq!(default.status, StatusCode::OK, "{}", default.body);
    assert_eq!(default.body["is_default"], true);
    let in_use = app.delete("/api/v1/series/GST%2026-", &asha.write()).await;
    assert_eq!(in_use.status, StatusCode::BAD_REQUEST, "a default stays");
    let old = app.delete("/api/v1/series/SINV-", &asha.write()).await;
    assert_eq!(old.status, StatusCode::NO_CONTENT);
    let gone = app.delete("/api/v1/series/SINV-", &asha.write()).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);

    // Books Users read number series but cannot change them.
    app.post(
        "/api/v1/users",
        &asha.write(),
        json!({"username": "ravi", "full_name": "Ravi", "password": "another long secret", "roles": ["books_user"]}),
    )
    .await;
    let ravi = Browser::from(
        &app.post(
            "/api/v1/session",
            &[],
            json!({"username": "ravi", "password": "another long secret"}),
        )
        .await,
    );
    assert_eq!(
        app.get("/api/v1/series", &ravi.read()).await.status,
        StatusCode::OK
    );
    let refused = app
        .post(
            "/api/v1/series",
            &ravi.write(),
            json!({"prefix": "R-", "reference_type": "payment"}),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.delete("/api/v1/series/PAY-", &ravi.write())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn parties_follow_frappe_roles_and_revisions() {
    let app = App::new();
    let asha = set_up_admin(&app).await;
    let setup = app
        .post(
            "/api/v1/company/setup",
            &asha.write(),
            json!({"company_name": "Shree Traders", "country": "India", "bank_name": "HDFC Bank"}),
        )
        .await;
    assert_eq!(setup.status, StatusCode::OK, "{}", setup.body);

    let created = app
        .post(
            "/api/v1/parties",
            &asha.write(),
            json!({
                "name": "Asha Stores", "role": "customer", "gst_type": "registered_regular",
                "gstin": "24aapfu0939f1zv",
                "address": {"address_line1": "12 Ring Road", "city": "Surat",
                            "state": "Gujarat", "country": "India", "postal_code": "395002"}
            }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let party: ot_api_types::Party = serde_json::from_value(created.body).unwrap();
    assert_eq!(party.gstin.as_deref(), Some("24AAPFU0939F1ZV"));
    assert_eq!(party.default_ledger_name.as_deref(), Some("Debtors"));
    let address = party.address.clone().unwrap();
    assert_eq!(address.place_of_supply.as_deref(), Some("Gujarat"));
    assert_eq!(
        address.display,
        "12 Ring Road, Surat, Gujarat, India, 395002"
    );

    // The shared input type round-trips through the API.
    let mut change = ot_api_types::PartyInput {
        name: party.name.clone(),
        role: "both".into(),
        address: party.address.clone(),
        gst_type: "consumer".into(),
        gstin: party.gstin.clone(),
        revision: Some(party.revision),
        ..Default::default()
    };
    let path = format!("/api/v1/parties/{}", party.id);
    let changed = app
        .post(&path, &asha.write(), serde_json::to_value(&change).unwrap())
        .await;
    assert_eq!(changed.status, StatusCode::OK, "{}", changed.body);
    assert_eq!(
        changed.body["gstin"],
        Value::Null,
        "a consumer has no GSTIN"
    );
    assert_eq!(changed.body["revision"], 2);
    let stale = app
        .post(&path, &asha.write(), serde_json::to_value(&change).unwrap())
        .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    change.revision = Some(2);
    change.name = String::new();
    let empty = app
        .post(&path, &asha.write(), serde_json::to_value(&change).unwrap())
        .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);

    let states = app
        .get("/api/v1/regional/indian-states", &asha.read())
        .await;
    assert!(
        states
            .body
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s == "Gujarat")
    );

    for (username, role) in [("chetan", "books_cashier"), ("veena", "viewer")] {
        app.post(
            "/api/v1/users",
            &asha.write(),
            json!({"username": username, "full_name": username, "password": "another long secret", "roles": [role]}),
        )
        .await;
    }
    let sign_in = |username: &'static str| {
        let app = &app;
        async move {
            Browser::from(
                &app.post(
                    "/api/v1/session",
                    &[],
                    json!({"username": username, "password": "another long secret"}),
                )
                .await,
            )
        }
    };
    let chetan = sign_in("chetan").await;
    let veena = sign_in("veena").await;
    // Cashiers add customers at the counter but do not delete them.
    let walk_in = app
        .post(
            "/api/v1/parties",
            &chetan.write(),
            json!({"name": "Walk-in", "role": "customer"}),
        )
        .await;
    assert_eq!(walk_in.status, StatusCode::CREATED, "{}", walk_in.body);
    assert_eq!(
        app.delete(&path, &chetan.write()).await.status,
        StatusCode::FORBIDDEN
    );
    // Viewers only read.
    assert_eq!(
        app.get("/api/v1/parties?role=customer", &veena.read())
            .await
            .body
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        app.post(
            "/api/v1/parties",
            &veena.write(),
            json!({"name": "X", "role": "customer"})
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.delete(&path, &asha.write()).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get(&path, &asha.read()).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn items_groups_and_units_follow_frappe_roles() {
    let app = App::new();
    let asha = set_up_admin(&app).await;
    app.post(
        "/api/v1/company/setup",
        &asha.write(),
        json!({"company_name": "Shree Traders", "country": "India", "bank_name": "HDFC Bank"}),
    )
    .await;

    let units: Vec<ot_api_types::Unit> =
        serde_json::from_value(app.get("/api/v1/units", &asha.read()).await.body).unwrap();
    assert_eq!(units.len(), 6);
    let bag = app
        .post(
            "/api/v1/units",
            &asha.write(),
            json!({"name": "Bag", "is_whole": true}),
        )
        .await;
    assert_eq!(bag.status, StatusCode::CREATED, "{}", bag.body);
    let group = app
        .post(
            "/api/v1/item-groups",
            &asha.write(),
            json!({"name": "Grains", "hsn_code": "1006"}),
        )
        .await;
    assert_eq!(group.status, StatusCode::CREATED, "{}", group.body);
    let bad_hsn = app
        .post(
            "/api/v1/item-groups",
            &asha.write(),
            json!({"name": "Bad", "hsn_code": "12A4"}),
        )
        .await;
    assert_eq!(bad_hsn.status, StatusCode::BAD_REQUEST);

    let input = ot_api_types::ItemInput {
        name: "Basmati Rice".into(),
        group_id: group.body["id"].as_i64(),
        usage: "both".into(),
        item_type: "product".into(),
        unit: Some("Kg".into()),
        rate: Some("125.5".into()),
        conversions: vec![ot_api_types::Conversion {
            unit: "Bag".into(),
            factor: "25".into(),
        }],
        ..Default::default()
    };
    let created = app
        .post(
            "/api/v1/items",
            &asha.write(),
            serde_json::to_value(&input).unwrap(),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let rice: ot_api_types::Item = serde_json::from_value(created.body).unwrap();
    assert_eq!(rice.rate, "125.50");
    assert_eq!(
        rice.hsn_code.as_deref(),
        Some("1006"),
        "taken from the group"
    );
    assert_eq!(rice.income_ledger_name, "Sales");
    assert_eq!(rice.expense_ledger_name, "Purchase");
    assert_eq!(rice.conversions[0].factor, "25");

    let path = format!("/api/v1/items/{}", rice.id);
    let stale = app
        .post(&path, &asha.write(), serde_json::to_value(&input).unwrap())
        .await;
    assert_eq!(stale.status, StatusCode::BAD_REQUEST, "no revision sent");
    let in_use = app.delete("/api/v1/units/Bag", &asha.write()).await;
    assert_eq!(in_use.status, StatusCode::BAD_REQUEST);
    let found = app.get("/api/v1/items?q=basmati", &asha.read()).await;
    assert_eq!(found.body.as_array().unwrap().len(), 1);

    for (username, role) in [("ravi", "books_user"), ("chetan", "books_cashier")] {
        app.post(
            "/api/v1/users",
            &asha.write(),
            json!({"username": username, "full_name": username, "password": "another long secret", "roles": [role]}),
        )
        .await;
    }
    let sign_in = |username: &'static str| {
        let app = &app;
        async move {
            Browser::from(
                &app.post(
                    "/api/v1/session",
                    &[],
                    json!({"username": username, "password": "another long secret"}),
                )
                .await,
            )
        }
    };
    let ravi = sign_in("ravi").await;
    let chetan = sign_in("chetan").await;
    // Books Users make items but not units; cashiers only read.
    let dal = app
        .post("/api/v1/items", &ravi.write(), json!({"name": "Toor Dal"}))
        .await;
    assert_eq!(dal.status, StatusCode::CREATED, "{}", dal.body);
    assert_eq!(
        app.post("/api/v1/units", &ravi.write(), json!({"name": "Box"}))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.get("/api/v1/items", &chetan.read()).await.status,
        StatusCode::OK
    );
    assert_eq!(
        app.post("/api/v1/items", &chetan.write(), json!({"name": "Sugar"}))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.delete(&path, &asha.write()).await.status,
        StatusCode::NO_CONTENT
    );
}
