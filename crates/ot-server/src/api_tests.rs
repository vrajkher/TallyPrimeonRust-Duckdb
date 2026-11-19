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
