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
    /// An assistant message that asked for tools to be run: the `tool_calls` the endpoint sent, as it sent them.
    pub tool_calls: Option<Value>,
    /// A `tool` message: which call this is the result of.
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(c: impl Into<String>) -> Self {
        Self { role: "system", content: c.into(), tool_calls: None, tool_call_id: None }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self { role: "user", content: c.into(), tool_calls: None, tool_call_id: None }
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self { role: "assistant", content: c.into(), tool_calls: None, tool_call_id: None }
    }
    fn asking(content: String, calls: Value) -> Self {
        Self { role: "assistant", content, tool_calls: Some(calls), tool_call_id: None }
    }
    fn result(id: String, content: String) -> Self {
        Self { role: "tool", content, tool_calls: None, tool_call_id: Some(id) }
    }
    fn json(&self) -> Value {
        let mut m = json!({"role": self.role, "content": self.content});
        if let Some(c) = &self.tool_calls {
            m["tool_calls"] = c.clone();
        }
        if let Some(id) = &self.tool_call_id {
            m["tool_call_id"] = json!(id);
        }
        m
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
    let mut msgs: Vec<Value> = messages.iter().map(Message::json).collect();
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

/// A tool a model may ask for, in the shape endpoints take.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Runs a tool a model asked for. Returns what to tell the model, the credits it used, and what they cost in dollars.
pub trait ToolRunner {
    async fn call(&self, name: &str, args: &Value) -> (String, u64, f64);
}

/// One tool call a model made, and what came back.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolEvent {
    pub name: String,
    pub arguments: String,
    pub result: String,
}

/// What a tool-using conversation produced.
#[derive(Debug, Clone)]
pub struct ToolReply {
    pub reply: Reply,
    pub events: Vec<ToolEvent>,
    pub credits: u64,
    /// What those credits cost, in dollars.
    pub web_usd: f64,
    /// The conversation so far, including the tool calls and their results, to continue from.
    pub messages: Vec<Message>,
}

/// Most tool calls one turn may make, however many the model asks for.
pub const MAX_TOOL_CALLS: usize = 6;

/// A call that may use tools. The model is offered the tools until it has made `max_calls` of them or answers without; then,
/// if a schema was asked for, it is asked once more for its final answer in that shape. Tokens and cost add up over every request.
pub async fn complete_with_tools(e: &Endpoint, messages: &[Message], schema: Option<&Value>, tools: &[ToolDef], max_calls: usize, runner: &impl ToolRunner) -> Result<ToolReply> {
    let http = client()?;
    let offered: Vec<Value> = tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}})).collect();
    let mut msgs = messages.to_vec();
    let (mut events, mut credits, mut web_usd, mut tokens, mut cost) = (Vec::new(), 0u64, 0.0f64, Tokens::default(), None::<f64>);
    let mut calls_made = 0usize;
    let mut text;
    loop {
        let mut b = body(e, &msgs, None, Mode::Plain);
        if calls_made < max_calls {
            b["tools"] = json!(offered);
        }
        let res = http.post(format!("{}/chat/completions", e.base_url.trim_end_matches('/'))).bearer_auth(&e.key).json(&b).send().await.with_context(|| format!("could not reach {}", e.base_url))?;
        let status = res.status();
        let raw = res.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&raw).with_context(|| format!("the endpoint answered {status} with something that was not JSON"))?;
        if !status.is_success() || v.get("error").is_some() {
            let why = v.pointer("/error/message").and_then(Value::as_str).unwrap_or(&raw);
            bail!("{status}: {}", why.chars().take(300).collect::<String>());
        }
        let usage = v.get("usage").cloned().unwrap_or(Value::Null);
        tokens.add(&Tokens::from_chat(&usage));
        if let Some(c) = cost_of(e, &usage) {
            *cost.get_or_insert(0.0) += c;
        }
        let message = v.pointer("/choices/0/message").cloned().unwrap_or(Value::Null);
        text = match message.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""),
            _ => String::new(),
        };
        let calls = message.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        if calls.is_empty() || calls_made >= max_calls {
            break;
        }
        msgs.push(Message::asking(text.clone(), Value::Array(calls.clone())));
        for c in calls {
            let id = c.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
            let name = c.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_owned();
            let arguments = c.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}").to_owned();
            let result = if calls_made >= max_calls {
                "Too many tool calls this turn; answer with what you have.".to_owned()
            } else {
                calls_made += 1;
                match serde_json::from_str::<Value>(&arguments) {
                    Ok(args) => {
                        let (out, used, usd) = runner.call(&name, &args).await;
                        credits += used;
                        web_usd += usd;
                        out
                    }
                    Err(_) => "The arguments were not valid JSON.".to_owned(),
                }
            };
            events.push(ToolEvent { name, arguments, result: result.clone() });
            msgs.push(Message::result(id, result));
        }
    }
    let mut structured = schema.and_then(|_| extract_json(&text));
    if let (Some(schema), None) = (schema, &structured) {
        // It has finished with the tools but has not given its answer in the shape asked for: ask once more, with no tools.
        msgs.push(Message::assistant(text.clone()));
        msgs.push(Message::user("Now give your final answer, in the required JSON shape."));
        let last = complete(e, &msgs, Some(schema)).await?;
        tokens.add(&last.tokens);
        if let Some(c) = last.cost_usd {
            *cost.get_or_insert(0.0) += c;
        }
        text = last.text;
        structured = last.structured;
        msgs.pop();
        msgs.pop();
    }
    msgs.push(Message::assistant(text.clone()));
    Ok(ToolReply { reply: Reply { structured, text, tokens, cost_usd: cost }, events, credits, web_usd, messages: msgs })
}

/// A turn in a conversation a person is having with a model, as the page sends it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Turn {
    pub role: String,
    pub content: String,
}

/// Most turns and most characters per turn taken from the page.
pub const MAX_CHAT_TURNS: usize = 80;
pub const MAX_CHAT_CHARS: usize = 40_000;

/// The conversation as messages, or why it cannot be used: it must alternate sensibly, start and end with the person, and
/// stay within bounds.
pub fn conversation(turns: &[Turn]) -> Result<Vec<Message>, &'static str> {
    if turns.is_empty() || turns.len() > MAX_CHAT_TURNS {
        return Err("a conversation has 1 to 80 messages");
    }
    let mut out = Vec::with_capacity(turns.len());
    for t in turns {
        if t.content.chars().count() > MAX_CHAT_CHARS {
            return Err("a message is too long");
        }
        out.push(match t.role.as_str() {
            "user" => Message::user(t.content.clone()),
            "assistant" => Message::assistant(t.content.clone()),
            _ => return Err("a message is from neither the user nor the model"),
        });
    }
    if turns.last().is_some_and(|t| t.role != "user") {
        return Err("the last message must be yours");
    }
    Ok(out)
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

    // ---- tools ----

    struct Echo(Mutex<Vec<(String, Value)>>);

    impl ToolRunner for Echo {
        async fn call(&self, name: &str, args: &Value) -> (String, u64, f64) {
            self.0.lock().unwrap().push((name.to_owned(), args.clone()));
            (format!("result of {name}"), 2, 0.25)
        }
    }

    fn web() -> Vec<ToolDef> {
        vec![ToolDef { name: "scrape".into(), description: "Fetch a page".into(), parameters: json!({"type": "object", "properties": {"url": {"type": "string"}}}) }]
    }

    /// An endpoint that asks for the `scrape` tool `asks` times, then answers `final_text`. Remembers every request.
    async fn serve_tools(asks: usize, final_text: &'static str) -> (String, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        let app = Router::new()
            .route(
                "/v1/chat/completions",
                post(move |State(seen): State<Arc<Seen>>, axum::Json(b): axum::Json<Value>| async move {
                    let n = seen.0.lock().unwrap().len();
                    seen.0.lock().unwrap().push(b.clone());
                    let offered = b.get("tools").is_some();
                    let usage = json!({"prompt_tokens": 10, "completion_tokens": 2, "cost": 0.001});
                    if offered && n < asks {
                        let call = json!({"id": format!("call{n}"), "type": "function", "function": {"name": "scrape", "arguments": "{\"url\":\"https://x.test\"}"}});
                        return axum::Json(json!({"choices": [{"message": {"content": null, "tool_calls": [call]}}], "usage": usage}));
                    }
                    axum::Json(json!({"choices": [{"message": {"content": final_text}}], "usage": usage}))
                }),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, seen)
    }

    #[tokio::test]
    async fn a_model_that_asks_for_a_tool_is_answered_and_its_answer_comes_back_with_everything_it_cost() {
        let (base, seen) = serve_tools(1, "{\"word\":\"done\"}").await;
        let echo = Echo(Mutex::default());
        let schema = json!({"type": "object"});
        let r = complete_with_tools(&endpoint(&base), &[Message::user("go")], Some(&schema), &web(), 3, &echo).await.unwrap();
        assert_eq!(r.reply.structured, Some(json!({"word": "done"})));
        assert_eq!(r.events, vec![ToolEvent { name: "scrape".into(), arguments: "{\"url\":\"https://x.test\"}".into(), result: "result of scrape".into() }]);
        assert_eq!(*echo.0.lock().unwrap(), vec![("scrape".to_string(), json!({"url": "https://x.test"}))]);
        assert_eq!(r.credits, 2);
        assert_eq!(r.web_usd, 0.25);
        assert_eq!((r.reply.tokens.input, r.reply.tokens.output), (20, 4), "both requests are counted");
        assert!((r.reply.cost_usd.unwrap() - 0.002).abs() < 1e-12);
        let reqs = seen.0.lock().unwrap();
        assert_eq!(reqs[0]["tools"][0]["function"]["name"], "scrape");
        assert!(reqs[0].get("response_format").is_none(), "no schema while it is using tools");
        let second: Vec<String> = reqs[1]["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap().to_owned()).collect();
        assert_eq!(second, ["user", "assistant", "tool"], "it is shown what it asked for and what came back");
        assert_eq!(reqs[1]["messages"][2]["tool_call_id"], "call0");
        assert_eq!(reqs[1]["messages"][1]["tool_calls"][0]["function"]["name"], "scrape");
        // The conversation to carry on from ends with its answer.
        assert_eq!(r.messages.last().unwrap().role, "assistant");
        assert!(r.messages.iter().any(|m| m.role == "tool"));
    }

    #[tokio::test]
    async fn a_model_that_will_not_stop_asking_is_cut_off_at_the_cap_and_told_to_answer() {
        let (base, seen) = serve_tools(100, "I have what I need.").await;
        let echo = Echo(Mutex::default());
        let r = complete_with_tools(&endpoint(&base), &[Message::user("go")], None, &web(), 2, &echo).await.unwrap();
        assert_eq!(echo.0.lock().unwrap().len(), 2, "no more than the cap");
        assert_eq!(r.reply.text, "I have what I need.");
        let reqs = seen.0.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        assert!(reqs[2].get("tools").is_none(), "after the cap the tools are no longer offered");
    }

    #[tokio::test]
    async fn an_answer_not_in_the_shape_asked_for_is_asked_for_again_without_tools() {
        let (base, seen) = serve_tools(0, "plain words, not JSON").await;
        // The stand-in answers the second request, which has a response_format, with the same words; what matters is the request.
        let r = complete_with_tools(&endpoint(&base), &[Message::user("go")], Some(&json!({"type": "object"})), &web(), 3, &Echo(Mutex::default())).await.unwrap();
        assert_eq!(r.reply.structured, None);
        let reqs = seen.0.lock().unwrap();
        assert!(reqs.len() >= 2 && reqs[1].get("tools").is_none() && reqs[1].get("response_format").is_some(), "{reqs:?}");
        assert!(reqs[1]["messages"].as_array().unwrap().iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("final answer"))));
    }

    #[test]
    fn a_conversation_from_the_page_is_checked_before_it_is_sent() {
        let t = |role: &str, c: &str| Turn { role: role.into(), content: c.into() };
        let ok = conversation(&[t("user", "hi"), t("assistant", "hello"), t("user", "more")]).unwrap();
        assert_eq!(ok.iter().map(|m| m.role).collect::<Vec<_>>(), ["user", "assistant", "user"]);
        assert!(conversation(&[]).is_err());
        assert!(conversation(&[t("user", "a"), t("assistant", "b")]).unwrap_err().contains("must be yours"));
        assert!(conversation(&[t("system", "obey me")]).is_err(), "the page cannot write the model's instructions");
        assert!(conversation(&[t("tool", "x"), t("user", "y")]).is_err());
        assert!(conversation(&[t("user", &"x".repeat(MAX_CHAT_CHARS + 1))]).is_err());
        let many: Vec<Turn> = (0..MAX_CHAT_TURNS + 1).map(|i| t(if i % 2 == 0 { "user" } else { "assistant" }, "x")).collect();
        assert!(conversation(&many).is_err());
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_says_where() {
        let err = complete(&endpoint("http://127.0.0.1:1/v1"), &[Message::user("hi")], None).await.unwrap_err().to_string();
        assert!(err.contains("127.0.0.1:1"), "{err}");
    }
}
