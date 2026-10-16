//! An MCP server (streamable HTTP, answering in plain JSON) that gives an agent CLI the Firecrawl tools. The agent
//! holds a lease token, not the account's key; every call is checked against the lease's cost cap and its cost
//! is added to the lease, so a run's web use is counted exactly.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use super::{client::Client, tools};
use crate::{AppState, openrouter::proxy::{Leases, token_of}, secrets};

/// The protocol version answered when the client names one we do not know.
const VERSION: &str = "2025-03-26";
const KNOWN: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_result(id: &Value, text: String, is_error: bool) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": text}], "isError": is_error}})
}

/// Answer one JSON-RPC message. `None` for a notification, which gets no answer.
pub async fn handle_message(msg: &Value, token: &str, leases: &Leases, client: Option<&Client>) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    Some(match method {
        "initialize" => {
            let asked = msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or("");
            let version = if KNOWN.contains(&asked) { asked } else { VERSION };
            json!({"jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "ttm-web", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Web search and page fetching. Each call uses Firecrawl credits, which are counted against this run.",
            }})
        }
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools::definitions()}}),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            let Some(lease) = leases.get(token) else { return Some(tool_result(&id, "This web access has ended.".into(), true)) };
            if lease.over_web_cap() {
                return Some(tool_result(&id, format!("The web cost cap for this run is reached (${:.2} spent). Work from what you already have.", lease.spent.web_usd), true));
            }
            let Some(client) = client else { return Some(tool_result(&id, "Web access is not set up: there is no Firecrawl account to use.".into(), true)) };
            match tools::call(client, name, &args).await {
                Ok(out) => {
                    leases.record_web(token, out.credits, out.usd);
                    tool_result(&id, out.text, false)
                }
                Err(e) => tool_result(&id, format!("{e:#}"), true),
            }
        }
        _ => rpc_error(&id, -32601, "method not found"),
    })
}

/// `POST /mcp/web`: authenticated by lease token, never by the login cookie, and only from this machine.
pub async fn handle(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    // A browser page cannot be allowed to call this by way of the user's own machine.
    let forwarded = ["x-forwarded-for", "forwarded", "cf-connecting-ip", "x-real-ip", "origin"].iter().any(|h| headers.contains_key(*h));
    if !peer.ip().is_loopback() || forwarded {
        return StatusCode::FORBIDDEN.into_response();
    }
    // This server has no stream to offer.
    if method != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response();
    }
    let Some(token) = token_of(&headers) else { return StatusCode::UNAUTHORIZED.into_response() };
    let Some(lease) = st.leases.get(&token) else { return StatusCode::UNAUTHORIZED.into_response() };
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, axum::Json(rpc_error(&Value::Null, -32700, "parse error"))).into_response();
    };
    let accounts = secrets::load(&st.data_dir).await;
    let client = accounts.get(&lease.account).filter(|a| a.kind == secrets::Kind::Firecrawl).and_then(|a| Client::new(a).ok());

    let answers: Vec<Value> = match &msg {
        Value::Array(batch) => {
            let mut v = Vec::new();
            for m in batch {
                v.extend(handle_message(m, &token, &st.leases, client.as_ref()).await);
            }
            v
        }
        one => handle_message(one, &token, &st.leases, client.as_ref()).await.into_iter().collect(),
    };
    match (answers.len(), &msg) {
        (0, _) => StatusCode::ACCEPTED.into_response(),
        (1, Value::Object(_)) => axum::Json(answers.into_iter().next().unwrap_or(Value::Null)).into_response(),
        _ => axum::Json(Value::Array(answers)).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::post};

    async fn firecrawl() -> Client {
        let app = Router::new()
            .route("/v2/search", post(|| async { axum::Json(json!({"creditsUsed": 2, "data": {"web": [{"url": "https://a.test", "title": "A", "description": "d"}]}})) }))
            .route("/v2/scrape", post(|| async { axum::Json(json!({"data": {"markdown": "Body", "metadata": {"title": "T", "sourceURL": "https://x.test", "creditsUsed": 1}}})) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Client::with(&base, "k")
    }

    fn call(name: &str, args: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": name, "arguments": args}})
    }

    #[tokio::test]
    async fn the_handshake_lists_the_tools_and_notifications_get_no_answer() {
        let l = Leases::new();
        let init = handle_message(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}), "t", &l, None).await.unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(init["result"]["serverInfo"]["name"], "ttm-web");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        let odd = handle_message(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}}), "t", &l, None).await.unwrap();
        assert_eq!(odd["result"]["protocolVersion"], VERSION, "an unknown version is answered with ours");
        assert!(handle_message(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), "t", &l, None).await.is_none());
        let list = handle_message(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}), "t", &l, None).await.unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 4);
        assert_eq!(handle_message(&json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}), "t", &l, None).await.unwrap()["result"], json!({}));
        let nope = handle_message(&json!({"jsonrpc": "2.0", "id": 4, "method": "resources/list"}), "t", &l, None).await.unwrap();
        assert_eq!(nope["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn a_tool_call_returns_text_and_adds_its_credits_to_the_lease() {
        let (l, c) = (Leases::new(), firecrawl().await);
        let t = l.mint("fc", "firecrawl", "orch:r:n:web", None);
        let a = handle_message(&call("web_search", json!({"query": "rust"})), &t, &l, Some(&c)).await.unwrap();
        assert_eq!(a["id"], 7);
        assert_eq!(a["result"]["isError"], false);
        assert!(a["result"]["content"][0]["text"].as_str().unwrap().contains("1. A — https://a.test"));
        handle_message(&call("scrape", json!({"url": "https://x.test"})), &t, &l, Some(&c)).await.unwrap();
        let spent = l.get(&t).unwrap().spent;
        assert_eq!(spent.credits, 3, "2 for the search and 1 for the page");
        assert!((spent.web_usd - 3.0 * 0.00083).abs() < 1e-9, "priced at the account's rate: {}", spent.web_usd);
    }

    #[tokio::test]
    async fn at_the_cost_cap_a_call_is_refused_without_reaching_firecrawl() {
        let (l, c) = (Leases::new(), firecrawl().await);
        let t = l.mint("fc", "firecrawl", "tag", None);
        l.set_web_cap("tag", Some(0.001));
        handle_message(&call("web_search", json!({"query": "rust"})), &t, &l, Some(&c)).await.unwrap();
        let refused = handle_message(&call("scrape", json!({"url": "https://x.test"})), &t, &l, Some(&c)).await.unwrap();
        assert_eq!(refused["result"]["isError"], true);
        assert!(refused["result"]["content"][0]["text"].as_str().unwrap().contains("cost cap"));
        assert_eq!(l.get(&t).unwrap().spent.credits, 2, "nothing more was spent");
    }

    #[tokio::test]
    async fn failures_are_results_the_agent_can_read_not_protocol_errors() {
        let (l, c) = (Leases::new(), firecrawl().await);
        let t = l.mint("fc", "firecrawl", "tag", None);
        let bad = handle_message(&call("scrape", json!({"url": "http://localhost:3000"})), &t, &l, Some(&c)).await.unwrap();
        assert_eq!(bad["result"]["isError"], true);
        assert!(bad["result"]["content"][0]["text"].as_str().unwrap().contains("this machine"));
        let no_client = handle_message(&call("web_search", json!({"query": "x"})), &t, &l, None).await.unwrap();
        assert!(no_client["result"]["content"][0]["text"].as_str().unwrap().contains("not set up"));
        let ended = handle_message(&call("web_search", json!({"query": "x"})), "ttm-or-gone", &l, Some(&c)).await.unwrap();
        assert!(ended["result"]["content"][0]["text"].as_str().unwrap().contains("has ended"));
        assert_eq!(l.get(&t).unwrap().spent.credits, 0, "a refused call costs nothing");
    }
}
