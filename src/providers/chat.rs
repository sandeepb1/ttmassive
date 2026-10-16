//! A direct chat-completions call to an OpenAI-compatible endpoint (OpenRouter included): no CLI, no tools. Used where
//! an agent's whole job is to read some text and answer in a shape: the helper calls that write a flow or a brief,
//! a classifier, and flow agents that need no files. The answer is asked for as JSON that matches a schema. Endpoints
//! differ in how much of that they support, so a refusal of `json_schema` falls back to plain JSON mode, then to
//! asking in words, and the answer is read out of whatever comes back.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::orch::usage::Tokens;
use crate::secrets::Prefs;

const TIMEOUT: Duration = Duration::from_secs(300);

/// Where to send the call, and what is known about what it costs.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub key: String,
    pub model: String,
    /// Dollars per million input and output tokens, when the account has them. Used when the reply carries no cost.
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
    /// OpenRouter: ask it for the charge, and pass the account's routing preferences.
    pub openrouter: bool,
    pub prefs: Prefs,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: &'static str,
    pub content: String,
}

impl Message {
    pub fn system(c: impl Into<String>) -> Self {
        Self { role: "system", content: c.into() }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self { role: "user", content: c.into() }
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self { role: "assistant", content: c.into() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    /// The answer as JSON, when a schema was asked for and the model gave one.
    pub structured: Option<Value>,
    pub text: String,
    pub tokens: Tokens,
    /// What the call cost: as charged when the reply says, else worked out from the account's prices, else unknown.
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    JsonSchema,
    JsonObject,
    Plain,
}

/// The request body for a call.
fn body(e: &Endpoint, messages: &[Message], schema: Option<&Value>, mode: Mode) -> Value {
    let mut msgs: Vec<Value> = messages.iter().map(|m| json!({"role": m.role, "content": m.content})).collect();
    if let (Some(schema), Mode::JsonObject | Mode::Plain) = (schema, mode) {
        // The endpoint cannot be made to follow the schema, so say it in words, as the last thing the model reads.
        msgs.push(json!({"role": "system", "content": format!("Reply with only one JSON object that matches this JSON Schema, and nothing else:\n{schema}")}));
    }
    let mut b = json!({"model": e.model, "messages": msgs, "stream": false});
    match (schema, mode) {
        (Some(schema), Mode::JsonSchema) => b["response_format"] = json!({"type": "json_schema", "json_schema": {"name": "answer", "strict": true, "schema": schema}}),
        (Some(_), Mode::JsonObject) => b["response_format"] = json!({"type": "json_object"}),
        _ => {}
    }
    if e.openrouter {
        b["usage"] = json!({"include": true});
        crate::openrouter::proxy::transform(&mut b, &e.model, &e.prefs);
    }
    b
}

/// The JSON in a model's reply: the reply itself, or the object inside a code fence or surrounded by chatter.
pub fn extract_json(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return v.is_object().then_some(v);
    }
    let unfenced = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).map(|r| r.trim_start()).and_then(|r| r.rsplit_once("```")).map(|(a, _)| a.trim());
    if let Some(v) = unfenced.and_then(|u| serde_json::from_str::<Value>(u).ok()).filter(Value::is_object) {
        return Some(v);
    }
    let (start, end) = (t.find('{')?, t.rfind('}')?);
    serde_json::from_str::<Value>(&t[start..=end]).ok().filter(Value::is_object)
}

/// What a reply cost, from the reply itself or else from the account's prices.
fn cost_of(e: &Endpoint, usage: &Value) -> Option<f64> {
    if let Some(c) = usage.get("cost").and_then(Value::as_f64).filter(|c| c.is_finite() && *c >= 0.0) {
        return Some(c);
    }
    let n = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0) as f64;
    match (e.price_in, e.price_out) {
        (Some(i), Some(o)) => Some((n("prompt_tokens") * i + n("completion_tokens") * o) / 1.0e6),
        _ => None,
    }
}

fn reply_from(e: &Endpoint, v: &Value, schema: Option<&Value>) -> Result<Reply> {
    let text = match v.pointer("/choices/0/message/content") {
        Some(Value::String(s)) => s.clone(),
        // Some providers answer with parts: `[{"type": "text", "text": "..."}]`.
        Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""),
        _ => bail!("the model sent no answer"),
    };
    let usage = v.get("usage").cloned().unwrap_or(Value::Null);
    Ok(Reply { structured: schema.and_then(|_| extract_json(&text)), tokens: Tokens::from_chat(&usage), cost_usd: cost_of(e, &usage), text })
}

/// Whether an error says the endpoint does not take the structured-output request, so the next mode is worth trying.
fn rejects_format(status: u16, text: &str) -> bool {
    let t = text.to_lowercase();
    matches!(status, 400 | 404 | 415 | 422) && ["response_format", "json_schema", "json_object", "structured", "schema", "not supported", "unsupported"].iter().any(|k| t.contains(k))
}

/// Where to call `model` on `account`, with the account's price for it if one was entered.
pub fn endpoint_for(account: &crate::secrets::Account, model: &str) -> Endpoint {
    let pinned = account.models.iter().find(|m| m.id == model);
    Endpoint {
        base_url: account.base_url.clone(),
        key: account.key.clone(),
        model: model.to_owned(),
        price_in: pinned.and_then(|m| m.price_in),
        price_out: pinned.and_then(|m| m.price_out),
        openrouter: account.kind == crate::secrets::Kind::Openrouter,
        prefs: account.prefs.clone(),
    }
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).timeout(TIMEOUT).build()?)
}

/// One call. With a schema, tries `json_schema`, then JSON mode, then plain words, and stops at the first the
/// endpoint accepts.
pub async fn complete(e: &Endpoint, messages: &[Message], schema: Option<&Value>) -> Result<Reply> {
    let http = client()?;
    let modes: &[Mode] = if schema.is_some() { &[Mode::JsonSchema, Mode::JsonObject, Mode::Plain] } else { &[Mode::Plain] };
    let mut last = String::new();
    for &mode in modes {
        let res = http
            .post(format!("{}/chat/completions", e.base_url.trim_end_matches('/')))
            .bearer_auth(&e.key)
            .json(&body(e, messages, schema, mode))
            .send()
            .await
            .with_context(|| format!("could not reach {}", e.base_url))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if status.is_success() {
            let v: Value = serde_json::from_str(&text).context("the endpoint's answer was not JSON")?;
            // Some gateways answer 200 with an error body.
            if let Some(msg) = v.pointer("/error/message").and_then(Value::as_str) {
                last = msg.to_owned();
                if rejects_format(400, msg) {
                    continue;
                }
                bail!("{msg}");
            }
            return reply_from(e, &v, schema);
        }
        let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(String::from)).unwrap_or_else(|| text.chars().take(300).collect());
        last = format!("{status}: {msg}");
        if !rejects_format(status.as_u16(), &msg) {
            bail!("{last}");
        }
    }
    bail!("the endpoint would not take the request: {last}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, extract::State, http::StatusCode, routing::post};
    use std::sync::{Arc, Mutex};

    fn endpoint(base: &str) -> Endpoint {
        Endpoint { base_url: base.into(), key: "k".into(), model: "vendor/model".into(), price_in: None, price_out: None, openrouter: false, prefs: Prefs::default() }
    }

    #[test]
    fn json_is_found_in_fences_and_chatter() {
        assert_eq!(extract_json("{\"a\":1}"), Some(json!({"a": 1})));
        assert_eq!(extract_json("```json\n{\"a\":1}\n```"), Some(json!({"a": 1})));
        assert_eq!(extract_json("```\n{\"a\":1}\n```"), Some(json!({"a": 1})));
        assert_eq!(extract_json("Sure! Here you go: {\"a\": {\"b\": 2}} Hope that helps."), Some(json!({"a": {"b": 2}})));
        assert_eq!(extract_json("[1,2]"), None, "an answer must be an object");
        assert_eq!(extract_json("no json here"), None);
        assert_eq!(extract_json("{broken"), None);
    }

    #[test]
    fn tokens_take_the_cached_part_out_of_the_prompt() {
        let t = Tokens::from_chat(&json!({"prompt_tokens": 100, "completion_tokens": 7, "prompt_tokens_details": {"cached_tokens": 60, "cache_write_tokens": 10}}));
        assert_eq!(t, Tokens { input: 30, output: 7, cache_read: 60, cache_write: 10 });
        assert_eq!(Tokens::from_chat(&json!({})), Tokens::default());
    }

    #[test]
    fn the_cost_is_the_charge_when_the_reply_has_one_and_the_price_otherwise() {
        let mut e = endpoint("x");
        let usage = json!({"prompt_tokens": 1000, "completion_tokens": 500});
        assert_eq!(cost_of(&e, &usage), None, "no price, no charge: unknown");
        e.price_in = Some(2.0);
        assert_eq!(cost_of(&e, &usage), None, "half a price is no price");
        e.price_out = Some(10.0);
        assert!((cost_of(&e, &usage).unwrap() - 0.007).abs() < 1e-12);
        assert_eq!(cost_of(&e, &json!({"prompt_tokens": 1, "cost": 0.5})), Some(0.5), "the charge wins over the price");
    }

    #[test]
    fn requests_ask_for_the_shape_the_mode_allows() {
        let schema = json!({"type": "object"});
        let m = [Message::system("s"), Message::user("u")];
        let b = body(&endpoint("x"), &m, Some(&schema), Mode::JsonSchema);
        assert_eq!(b["response_format"]["json_schema"]["strict"], true);
        assert_eq!(b["messages"].as_array().unwrap().len(), 2);
        let b = body(&endpoint("x"), &m, Some(&schema), Mode::JsonObject);
        assert_eq!(b["response_format"]["type"], "json_object");
        assert!(b["messages"][2]["content"].as_str().unwrap().contains("JSON Schema"), "the schema is put in words");
        let b = body(&endpoint("x"), &m, None, Mode::Plain);
        assert!(b.get("response_format").is_none() && b["messages"].as_array().unwrap().len() == 2);
        let mut or = endpoint("x");
        or.openrouter = true;
        or.prefs = Prefs { sort: Some("price".into()), ..Prefs::default() };
        let b = body(&or, &m, None, Mode::Plain);
        assert_eq!((b["usage"]["include"].clone(), b["provider"]["sort"].clone()), (json!(true), json!("price")));
        assert!(body(&endpoint("x"), &m, None, Mode::Plain).get("usage").is_none(), "other endpoints are not sent OpenRouter's options");
    }

    #[test]
    fn only_a_format_complaint_is_worth_another_try() {
        assert!(rejects_format(400, "response_format json_schema is not supported by this model"));
        assert!(rejects_format(422, "Unsupported parameter: response_format"));
        assert!(!rejects_format(401, "invalid api key"), "a bad key is not a format problem");
        assert!(!rejects_format(400, "context length exceeded"));
        assert!(!rejects_format(500, "schema"));
    }

    // ---- against a stand-in endpoint ----

    #[derive(Default)]
    struct Seen(Mutex<Vec<Value>>);

    /// An endpoint that rejects any request whose `response_format` type is in `refuse`.
    async fn serve(refuse: &'static [&'static str], content: &'static str) -> (String, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        let app = Router::new()
            .route(
                "/v1/chat/completions",
                post(move |State(seen): State<Arc<Seen>>, axum::Json(b): axum::Json<Value>| async move {
                    let kind = b.pointer("/response_format/type").and_then(Value::as_str).unwrap_or("").to_owned();
                    seen.0.lock().unwrap().push(b);
                    if refuse.contains(&kind.as_str()) {
                        return (StatusCode::BAD_REQUEST, axum::Json(json!({"error": {"message": format!("response_format {kind} is not supported")}})));
                    }
                    (StatusCode::OK, axum::Json(json!({"choices": [{"message": {"content": content}}], "usage": {"prompt_tokens": 20, "completion_tokens": 5}})))
                }),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, seen)
    }

    #[tokio::test]
    async fn an_endpoint_that_takes_json_schema_is_asked_once() {
        let (base, seen) = serve(&[], "{\"word\":\"ok\"}").await;
        let schema = json!({"type": "object", "properties": {"word": {"type": "string"}}});
        let mut e = endpoint(&base);
        e.price_in = Some(1.0);
        e.price_out = Some(1.0);
        let r = complete(&e, &[Message::user("hi")], Some(&schema)).await.unwrap();
        assert_eq!(r.structured, Some(json!({"word": "ok"})));
        assert_eq!((r.tokens.input, r.tokens.output), (20, 5));
        assert!((r.cost_usd.unwrap() - 0.000025).abs() < 1e-12);
        assert_eq!(seen.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_endpoint_that_refuses_the_structured_formats_is_asked_in_words_and_its_chatter_is_read() {
        let (base, seen) = serve(&["json_schema", "json_object"], "Sure!\n```json\n{\"word\":\"ok\"}\n```").await;
        let r = complete(&endpoint(&base), &[Message::user("hi")], Some(&json!({"type": "object"}))).await.unwrap();
        assert_eq!(r.structured, Some(json!({"word": "ok"})));
        let kinds: Vec<String> = seen.0.lock().unwrap().iter().map(|b| b.pointer("/response_format/type").and_then(Value::as_str).unwrap_or("none").to_owned()).collect();
        assert_eq!(kinds, ["json_schema", "json_object", "none"], "each mode tried once, in order");
        assert!(seen.0.lock().unwrap()[2]["messages"].as_array().unwrap().iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("JSON Schema"))));
    }

    #[tokio::test]
    async fn without_a_schema_the_answer_is_just_text() {
        let (base, seen) = serve(&[], "hello there").await;
        let r = complete(&endpoint(&base), &[Message::user("hi")], None).await.unwrap();
        assert_eq!((r.text.as_str(), r.structured), ("hello there", None));
        assert!(seen.0.lock().unwrap()[0].get("response_format").is_none());
    }

    #[tokio::test]
    async fn a_real_error_is_reported_and_not_retried() {
        let app = Router::new().route("/v1/chat/completions", post(|| async { (StatusCode::UNAUTHORIZED, axum::Json(json!({"error": {"message": "invalid api key"}}))) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let err = complete(&endpoint(&base), &[Message::user("hi")], Some(&json!({"type": "object"}))).await.unwrap_err().to_string();
        assert!(err.contains("401") && err.contains("invalid api key"), "{err}");
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_says_where() {
        let err = complete(&endpoint("http://127.0.0.1:1/v1"), &[Message::user("hi")], None).await.unwrap_err().to_string();
        assert!(err.contains("127.0.0.1:1"), "{err}");
    }
}
