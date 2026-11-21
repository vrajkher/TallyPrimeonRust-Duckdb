//! `opentally mcp`: a stdio MCP server for desktop clients that relays each message to a
//! running OpenTally's `/mcp` endpoint.
//!
//! DuckDB lets one process write to a book, so the bridge does not open the book
//! itself; the server must be running, as for the Node connector.
//!
//! Configuration, as for the Node connector:
//! - `OPENTALLY_PORT` (default 3000), or `--server URL` for another address;
//! - `OPENTALLY_API_TOKEN` once the book has users;
//! - `OPENTALLY_MCP_WRITES=1` to enable write tools.

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::WRITES_HEADER;

pub struct BridgeConfig {
    pub endpoint: String,
    pub token: Option<String>,
    pub writes: bool,
}

impl BridgeConfig {
    pub fn from_environment(server: Option<&str>) -> Result<Self, String> {
        let base = match server {
            Some(url) => url.trim_end_matches('/').to_owned(),
            None => {
                let port = std::env::var("OPENTALLY_PORT")
                    .or_else(|_| std::env::var("VOUCHER_PORT"))
                    .unwrap_or_else(|_| "3000".into());
                let port: u16 = port.parse().map_err(|_| {
                    "OPENTALLY_PORT must be a TCP port between 1 and 65535.".to_owned()
                })?;
                format!("http://127.0.0.1:{port}")
            }
        };
        let token = std::env::var("OPENTALLY_API_TOKEN")
            .ok()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty());
        if token
            .as_deref()
            .is_some_and(|t| !t.starts_with(crate::auth::TOKEN_PREFIX))
        {
            return Err(
                "OPENTALLY_API_TOKEN must be an OpenTally API token starting with ot_.".into(),
            );
        }
        let writes = std::env::var("OPENTALLY_MCP_WRITES")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        Ok(Self {
            endpoint: format!("{base}/mcp"),
            token,
            writes,
        })
    }
}

/// A JSON-RPC error for a request the server could not answer.
fn failure(message: &Value, text: &str) -> Option<String> {
    let id = message.get("id")?;
    // Only requests (with an id and a method) get a reply; notifications do not.
    message.get("method")?;
    Some(
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32001, "message": text}}).to_string(),
    )
}

/// Relays stdin to the server until stdin closes.
pub async fn run(config: BridgeConfig) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(75))
        .build()
        .map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    let mut protocol_version: Option<String> = None;
    let mut session: Option<String> = None;

    while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(_) => {
                let error = json!({"jsonrpc": "2.0", "id": null,
                    "error": {"code": -32700, "message": "Parse error"}});
                stdout
                    .write_all(format!("{error}\n").as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                continue;
            }
        };
        let mut request = client
            .post(&config.endpoint)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(line.clone());
        if let Some(token) = &config.token {
            request = request.bearer_auth(token);
        }
        if config.writes {
            request = request.header(WRITES_HEADER, "1");
        }
        if let Some(version) = &protocol_version {
            request = request.header("mcp-protocol-version", version);
        }
        if let Some(id) = &session {
            request = request.header("mcp-session-id", id);
        }

        let replies: Vec<String> = match request.send().await {
            Err(_) => failure(
                &message,
                &format!(
                    "Could not reach OpenTally at {}. Start it, check OPENTALLY_PORT, and retry.",
                    config.endpoint
                ),
            )
            .into_iter()
            .collect(),
            Ok(response) => {
                if let Some(id) = response
                    .headers()
                    .get("mcp-session-id")
                    .and_then(|v| v.to_str().ok())
                {
                    session = Some(id.to_owned());
                }
                let status = response.status();
                let is_stream = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.starts_with("text/event-stream"));
                let body = response.text().await.unwrap_or_default();
                if status.is_success() {
                    let bodies: Vec<String> = if is_stream {
                        body.lines()
                            .filter_map(|l| l.strip_prefix("data:"))
                            .map(|l| l.trim().to_owned())
                            .filter(|l| !l.is_empty())
                            .collect()
                    } else if body.trim().is_empty() {
                        Vec::new()
                    } else {
                        vec![body]
                    };
                    for reply in &bodies {
                        if let Ok(value) = serde_json::from_str::<Value>(reply)
                            && let Some(version) = value["result"]["protocolVersion"].as_str()
                        {
                            protocol_version = Some(version.to_owned());
                        }
                    }
                    bodies
                        .into_iter()
                        .map(|b| serde_json::from_str::<Value>(&b).map_or(b, |v| v.to_string()))
                        .collect()
                } else {
                    let reason = serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|v| {
                            v["error"]
                                .as_str()
                                .map(str::to_owned)
                                .or_else(|| v["error"]["message"].as_str().map(str::to_owned))
                        })
                        .unwrap_or_else(|| format!("OpenTally returned HTTP {status}."));
                    let hint = if status.as_u16() == 401 {
                        " Set OPENTALLY_API_TOKEN to an OpenTally API token."
                    } else {
                        ""
                    };
                    failure(&message, &format!("{reason}{hint}"))
                        .into_iter()
                        .collect()
                }
            }
        };
        for reply in replies {
            stdout
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .map_err(|e| e.to_string())?;
        }
        stdout.flush().await.map_err(|e| e.to_string())?;
    }
    Ok(())
}
