//! Drives Codex agents through one `codex app-server` process (JSON-RPC over stdio), one thread per
//! node. Wire shapes were checked against the schema `codex app-server generate-json-schema` writes
//! for codex-cli 0.162.1; re-check after a Codex upgrade, the protocol is marked experimental.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, broadcast, oneshot};

use super::trace::{Kind, Trace, clip, compact};
use super::usage::Tokens;
use super::{Outcome, Proxy};

#[derive(Debug, Clone)]
pub struct Spec {
    pub model: String,
    pub system_prompt: String,
    pub read_only: bool,
    pub cwd: PathBuf,
    /// Run on OpenRouter through ttm's proxy instead of your own Codex login.
    pub proxy: Option<Proxy>,
    /// Give the agent the web tools, through ttm's MCP endpoint with this lease.
    pub web: Option<Proxy>,
}

pub fn initialize_params() -> Value {
    json!({"clientInfo": {"name": "ttm-orch", "title": "ttm", "version": env!("CARGO_PKG_VERSION")}})
}

/// Codex features that an agent in a flow does not need and should not have. The first three bring the user's
/// own plugins, hooks and apps into the prompt. The rest add tools the flow does not use: some cost tokens
/// (together about 700 per prompt), and several are a risk in an unattended agent: it could drive the user's
/// browser or computer, or start agents of its own outside the flow.
const FEATURES_OFF: [&str; 12] = [
    "plugins", "hooks", "apps", "multi_agent", "goals", "image_generation", "browser_use", "computer_use", "in_app_browser",
    "tool_suggest", "skill_search", "sleep_tool",
];

/// Names of the MCP servers the user's own Codex config defines, from a `config/read` result.
pub fn mcp_names(config_read: &Value) -> Vec<String> {
    let cfg = config_read.get("config").unwrap_or(config_read);
    cfg.get("mcp_servers")
        .or_else(|| cfg.get("mcpServers"))
        .and_then(Value::as_object)
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// `mcp_off` are servers to switch off for this thread: each one lists its tools in every prompt, which
/// costs tokens and changes the cached prefix, and an agent in a flow does not need them.
pub fn thread_start_params(spec: &Spec, mcp_off: &[String]) -> Value {
    let features: serde_json::Map<String, Value> = FEATURES_OFF.iter().map(|f| (f.to_string(), Value::Bool(false))).collect();
    let mut config = json!({ "features": features });
    let mut servers: serde_json::Map<String, Value> = mcp_off.iter().map(|n| (n.clone(), json!({"enabled": false}))).collect();
    if let Some(w) = &spec.web {
        // This thread's own copy of the server, with its own lease; the server process is shared, so the token cannot be an environment variable.
        servers.insert(
            "ttm_web".into(),
            json!({"url": format!("{}/mcp/web", w.origin.trim_end_matches('/')), "http_headers": {"Authorization": format!("Bearer {}", w.token)}, "enabled": true, "startup_timeout_sec": 20, "tool_timeout_sec": 120}),
        );
    }
    if !servers.is_empty() {
        config["mcp_servers"] = servers.into();
    }
    if !spec.read_only {
        // A builder installs dependencies and runs the project's checks, which need the network. Its sandbox lets
        // it write only inside its own copy, but Codex shuts the network in that sandbox unless told otherwise.
        config["sandbox_workspace_write"] = json!({ "network_access": true });
    }
    if let Some(p) = &spec.proxy {
        // This thread's own provider. The token rides in a header because the server is shared by every node, so an
        // environment variable could hold only one of their leases.
        config["model_provider"] = json!("ttm_or");
        config["model_providers"] = json!({"ttm_or": {
            "name": "OpenRouter via ttm",
            "base_url": format!("{}/or/openai/v1", p.origin.trim_end_matches('/')),
            "wire_api": "responses",
            "http_headers": {"Authorization": format!("Bearer {}", p.token)},
        }});
    }
    json!({
        "cwd": spec.cwd,
        "model": spec.model,
        // Extra instructions on top of Codex's own, so its built-in prompt stays as it was shipped.
        "developerInstructions": spec.system_prompt,
        // Nobody is there to answer an approval prompt; the sandbox is the limit instead.
        "approvalPolicy": "never",
        "sandbox": if spec.read_only { "read-only" } else { "workspace-write" },
        // Keep the user's own plugins, hooks and apps out of the agent: they start MCP servers and
        // add tools to every prompt, which costs tokens and changes the cached prefix.
        "config": config,
    })
}

pub fn turn_start_params(thread_id: &str, prompt: &str, schema: &Value) -> Value {
    json!({
        "threadId": thread_id,
        "input": [{"type": "text", "text": prompt}],
        "outputSchema": schema,
    })
}

/// Collects one turn's notifications into an outcome. Pure, so it is tested with recorded shapes.
pub struct TurnAcc {
    thread_id: String,
    /// The thread's running total before this turn; usage reports are totals.
    base: Tokens,
    latest_total: Option<Tokens>,
    text: String,
}

impl TurnAcc {
    pub fn new(thread_id: &str, base: Tokens) -> Self {
        Self { thread_id: thread_id.into(), base, latest_total: None, text: String::new() }
    }

    /// The thread's running total after the last report seen, to be the next turn's base.
    pub fn total(&self) -> Tokens {
        self.latest_total.unwrap_or(self.base)
    }

    /// Feed one notification. Returns the result once the turn ends.
    pub fn feed(&mut self, note: &Value) -> Option<Result<Outcome, String>> {
        let p = note.get("params")?;
        if p.get("threadId").and_then(Value::as_str) != Some(self.thread_id.as_str()) {
            return None;
        }
        match note.get("method").and_then(Value::as_str)? {
            "thread/tokenUsage/updated" => {
                if let Some(t) = p.pointer("/tokenUsage/total") {
                    self.latest_total = Some(Tokens::from_codex(t));
                }
                None
            }
            "item/completed" => {
                if p.pointer("/item/type").and_then(Value::as_str) == Some("agentMessage") {
                    if let Some(t) = p.pointer("/item/text").and_then(Value::as_str) {
                        self.text = t.to_string();
                    }
                }
                None
            }
            "error" if p.get("willRetry") == Some(&Value::Bool(false)) => {
                let m = p.pointer("/error/message").and_then(Value::as_str).unwrap_or("codex reported an error");
                Some(Err(m.to_string()))
            }
            "turn/completed" => {
                let status = p.pointer("/turn/status").and_then(Value::as_str).unwrap_or("");
                if status != "completed" {
                    let why = p.pointer("/turn/error/message").and_then(Value::as_str).unwrap_or(status);
                    return Some(Err(format!("turn {status}: {why}")));
                }
                let structured = serde_json::from_str::<Value>(&self.text).ok();
                Some(Ok(Outcome {
                    structured,
                    text: std::mem::take(&mut self.text),
                    tokens: self.total().since(&self.base),
                    cost_usd: 0.0,
                    credits: 0,
                    is_error: false,
                    session: Some(self.thread_id.clone()),
                }))
            }
            _ => None,
        }
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// One running `codex app-server`.
pub struct Server {
    _child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next: AtomicU64,
    notes: broadcast::Sender<Value>,
    /// The user's own MCP servers, switched off in every thread.
    mcp_servers: Vec<String>,
    /// False once the process has ended, so the next use starts a new one.
    alive: Arc<std::sync::atomic::AtomicBool>,
}

impl Server {
    pub async fn start() -> Result<Server> {
        let mut child = Command::new("codex")
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("could not start codex")?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().context("no stdin")?));
        let stdout = child.stdout.take().context("no stdout")?;
        let pending: Pending = Arc::default();
        let (notes, _) = broadcast::channel(1024);

        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (p, n, w, a) = (pending.clone(), notes.clone(), stdin.clone(), alive.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                let id = v.get("id").and_then(Value::as_u64);
                let method = v.get("method").is_some();
                match (id, method) {
                    // A reply to one of our requests.
                    (Some(id), false) => {
                        if let Some(tx) = p.lock().await.remove(&id) {
                            let r = match v.get("error") {
                                Some(e) => Err(e.get("message").and_then(Value::as_str).unwrap_or("error").to_string()),
                                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(r);
                        }
                    }
                    // A request from the server (an approval). Nobody can answer, so refuse it
                    // rather than leave the turn hanging.
                    (Some(id), true) => {
                        let reply = json!({"id": id, "error": {"code": -32601, "message": "not supported"}});
                        let mut w = w.lock().await;
                        let _ = w.write_all(format!("{reply}\n").as_bytes()).await;
                        let _ = w.flush().await;
                    }
                    (None, true) => {
                        let _ = n.send(v);
                    }
                    _ => {}
                }
            }
            // The process ended: fail everything still waiting, and let the next use start a new one.
            a.store(false, Ordering::SeqCst);
            for (_, tx) in p.lock().await.drain() {
                let _ = tx.send(Err("codex exited".into()));
            }
        });

        let mut server = Server { _child: child, stdin, pending, next: AtomicU64::new(1), notes, mcp_servers: Vec::new(), alive };
        server.request("initialize", initialize_params()).await?;
        server.send(&json!({"method": "initialized"})).await?;
        // Which MCP servers there are is the user's business; if it cannot be read, nothing is switched off.
        if let Ok(cfg) = server.request("config/read", json!({"includeLayers": false})).await {
            server.mcp_servers = mcp_names(&cfg);
        }
        Ok(server)
    }

    async fn send(&self, v: &Value) -> Result<()> {
        let mut w = self.stdin.lock().await;
        w.write_all(format!("{v}\n").as_bytes()).await?;
        w.flush().await?;
        Ok(())
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.send(&json!({"id": id, "method": method, "params": params})).await?;
        rx.await.map_err(|_| anyhow!("codex closed"))?.map_err(|e| anyhow!("{method}: {e}"))
    }

    /// Models this account can use, read live so names never go stale.
    pub async fn models(&self) -> Result<Vec<String>> {
        let r = self.request("model/list", json!({})).await?;
        Ok(r.get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("model").and_then(Value::as_str).map(String::from))
            .collect())
    }

    pub async fn start_thread(&self, spec: &Spec) -> Result<Thread> {
        let r = self.request("thread/start", thread_start_params(spec, &self.mcp_servers)).await?;
        let id = r.pointer("/thread/id").and_then(Value::as_str).context("no thread id")?.to_string();
        Ok(Thread { id, total: Tokens::default() })
    }

    /// Run one turn on a thread and wait for it to finish. `on_note` sees every notification.
    pub async fn turn(
        &self,
        thread: &mut Thread,
        prompt: &str,
        schema: &Value,
        mut on_note: impl FnMut(&Value),
    ) -> Result<Outcome> {
        // Subscribe first, so nothing between sending the turn and listening is missed.
        let mut rx = self.notes.subscribe();
        let mut acc = TurnAcc::new(&thread.id, thread.total);
        self.request("turn/start", turn_start_params(&thread.id, prompt, schema)).await?;
        loop {
            let note = match rx.recv().await {
                Ok(n) => n,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => bail!("codex exited before the turn ended"),
            };
            on_note(&note);
            if let Some(done) = acc.feed(&note) {
                thread.total = acc.total();
                return done.map_err(|e| anyhow!(e));
            }
        }
    }

    /// Stop the turn in progress on a thread.
    pub async fn interrupt(&self, thread_id: &str) {
        let _ = self.request("turn/interrupt", json!({"threadId": thread_id})).await;
    }
}

/// One shared server, started the first time a Codex node needs it, and started again if it has since ended
/// (it was killed, or Codex crashed): without that, every Codex agent would fail until ttm itself restarted.
#[derive(Default)]
pub struct Hub(Mutex<Option<Arc<Server>>>);

impl Hub {
    pub async fn get(&self) -> Result<Arc<Server>> {
        let mut slot = self.0.lock().await;
        if let Some(s) = slot.as_ref().filter(|s| s.alive.load(Ordering::SeqCst)) {
            return Ok(s.clone());
        }
        let server = Arc::new(Server::start().await?);
        *slot = Some(server.clone());
        Ok(server)
    }
}

/// A short line for the live transcript about what a notification says the agent is doing.
pub fn describe(note: &Value) -> Option<String> {
    if note.get("method").and_then(Value::as_str) != Some("item/completed") {
        return None;
    }
    let item = note.pointer("/params/item")?;
    let s = |k: &str| item.get(k).and_then(Value::as_str);
    match s("type")? {
        "commandExecution" => Some(format!("→ ran {}", s("command").unwrap_or("a command"))),
        "fileChange" => Some("→ changed files".into()),
        "webSearch" => Some(format!("→ searched {}", s("query").unwrap_or("the web"))),
        "mcpToolCall" | "dynamicToolCall" => Some(format!("→ used {}", s("tool").unwrap_or("a tool"))),
        _ => None,
    }
}

/// Record what a notification shows into a turn's trace. What the agent said and its reasoning are kept as text; every
/// other finished item (a command, a file change, a web search, a tool call) is kept as the item itself, which holds the
/// command, its output and its exit status whatever the item is called.
pub fn trace_note(note: &Value, out: &mut Trace) {
    match note.get("method").and_then(Value::as_str) {
        Some("item/completed") => {
            let Some(item) = note.pointer("/params/item") else { return };
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("item");
            match kind {
                "agentMessage" => out.push(Kind::Say, "", item.get("text").and_then(Value::as_str).unwrap_or("")),
                "reasoning" => {
                    let text = item.get("summary").map(compact).filter(|t| !t.is_empty() && t != "[]").unwrap_or_default();
                    if !text.is_empty() {
                        out.push(Kind::Note, "reasoning", &text);
                    }
                }
                "userMessage" => {}
                _ => {
                    let title = item.get("tool").and_then(Value::as_str).or_else(|| item.get("command").and_then(Value::as_str)).unwrap_or(kind);
                    let mut shown = item.clone();
                    if let Some(o) = shown.as_object_mut() {
                        for noise in ["id", "type", "status"] {
                            o.remove(noise);
                        }
                    }
                    out.push(Kind::Tool, clip(title, 80), &compact(&shown));
                }
            }
        }
        Some("error") => {
            let msg = note.pointer("/params/error/message").and_then(Value::as_str).unwrap_or("an error");
            out.push(Kind::Note, "error", msg);
        }
        _ => {}
    }
}

/// One Codex node's conversation.
pub struct Thread {
    pub id: String,
    /// Running total for the thread, the base for the next turn's usage.
    total: Tokens,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(method: &str, params: Value) -> Value {
        json!({"method": method, "params": params})
    }

    fn usage(input: u64, cached: u64, output: u64) -> Value {
        note(
            "thread/tokenUsage/updated",
            json!({"threadId": "t1", "turnId": "u1", "tokenUsage": {
                "total": {"inputTokens": input, "cachedInputTokens": cached, "cacheWriteInputTokens": 0,
                          "outputTokens": output, "reasoningOutputTokens": 0, "totalTokens": input + output},
                "last": {"inputTokens": 1, "cachedInputTokens": 0, "cacheWriteInputTokens": 0,
                         "outputTokens": 1, "reasoningOutputTokens": 0, "totalTokens": 2},
                "modelContextWindow": 200000}}),
        )
    }

    fn agent_message(text: &str) -> Value {
        note(
            "item/completed",
            json!({"threadId": "t1", "turnId": "u1", "completedAtMs": 1,
                   "item": {"type": "agentMessage", "id": "i1", "text": text}}),
        )
    }

    fn completed(status: &str) -> Value {
        note("turn/completed", json!({"threadId": "t1", "turn": {"id": "u1", "items": [], "status": status}}))
    }

    #[test]
    fn the_trace_keeps_what_codex_said_and_every_item_it_finished() {
        let done = |item: Value| json!({"method": "item/completed", "params": {"threadId": "t", "item": item}});
        let mut t = Trace::default();
        for n in [
            done(json!({"type": "agentMessage", "id": "1", "text": "Checking the file."})),
            done(json!({"type": "commandExecution", "id": "2", "status": "completed", "command": "ls -la", "aggregatedOutput": "total 0", "exitCode": 0})),
            done(json!({"type": "mcpToolCall", "id": "3", "tool": "scrape", "arguments": {"url": "https://x.test"}})),
            done(json!({"type": "reasoning", "id": "4", "summary": []})),
            done(json!({"type": "userMessage", "text": "the prompt"})),
            json!({"method": "error", "params": {"error": {"message": "rate limited"}, "willRetry": true}}),
            json!({"method": "thread/tokenUsage/updated", "params": {}}),
        ] {
            trace_note(&n, &mut t);
        }
        let e = t.into_entries();
        assert_eq!(e.len(), 4, "{e:?}");
        assert_eq!((e[0].kind, e[0].body.as_str()), (Kind::Say, "Checking the file."));
        assert_eq!((e[1].kind, e[1].title.as_str()), (Kind::Tool, "ls -la"));
        assert!(e[1].body.contains("total 0") && e[1].body.contains("\"exitCode\":0") && !e[1].body.contains("\"id\""), "{}", e[1].body);
        assert_eq!(e[2].title, "scrape");
        assert!(e[2].body.contains("https://x.test"));
        assert_eq!((e[3].kind, e[3].body.as_str()), (Kind::Note, "rate limited"));
    }

    #[test]
    fn a_thread_with_web_access_gets_the_server_with_its_own_token_next_to_the_servers_switched_off() {
        let spec = |token: &str| Spec {
            model: "gpt-6-luna".into(),
            system_prompt: String::new(),
            read_only: true,
            cwd: PathBuf::from("/tmp"),
            proxy: None,
            web: Some(Proxy { origin: "http://127.0.0.1:7681".into(), token: token.into() }),
        };
        let a = thread_start_params(&spec("web-a"), &["homelab".to_string()]);
        let servers = &a["config"]["mcp_servers"];
        assert_eq!(servers["homelab"]["enabled"], false, "the user's own servers stay off");
        assert_eq!(servers["ttm_web"]["url"], "http://127.0.0.1:7681/mcp/web");
        assert_eq!(servers["ttm_web"]["http_headers"]["Authorization"], "Bearer web-a");
        assert_eq!(servers["ttm_web"]["enabled"], true);
        let b = thread_start_params(&spec("web-b"), &[]);
        assert_eq!(b["config"]["mcp_servers"]["ttm_web"]["http_headers"]["Authorization"], "Bearer web-b", "each thread its own");
        let none = thread_start_params(&Spec { web: None, ..spec("x") }, &[]);
        assert!(none["config"].get("mcp_servers").is_none(), "nothing added when there is no web access");
    }

    #[test]
    fn a_thread_on_openrouter_gets_its_own_provider_with_its_own_lease() {
        let spec = |token: &str| Spec {
            model: "openai/gpt-6-luna".into(),
            system_prompt: String::new(),
            read_only: true,
            cwd: PathBuf::from("/tmp"),
            proxy: Some(Proxy { origin: "http://127.0.0.1:7681/".into(), token: token.into() }),
            web: None,
        };
        let a = thread_start_params(&spec("ttm-or-a"), &[]);
        let b = thread_start_params(&spec("ttm-or-b"), &[]);
        assert_eq!(a["config"]["model_provider"], "ttm_or");
        let p = &a["config"]["model_providers"]["ttm_or"];
        assert_eq!((p["base_url"].as_str(), p["wire_api"].as_str()), (Some("http://127.0.0.1:7681/or/openai/v1"), Some("responses")));
        assert_eq!(p["http_headers"]["Authorization"], "Bearer ttm-or-a");
        assert_eq!(b["config"]["model_providers"]["ttm_or"]["http_headers"]["Authorization"], "Bearer ttm-or-b", "each thread has its own token");
        assert_eq!(a["model"], "openai/gpt-6-luna");
        assert!(a["config"]["features"]["plugins"] == false, "the isolation settings are still there");
        // An agent on your own plan is left on your own Codex login.
        let own = thread_start_params(&Spec { proxy: None, ..spec("x") }, &[]);
        assert!(own["config"].get("model_provider").is_none() && own["config"].get("model_providers").is_none());
    }

    #[test]
    fn a_turn_collects_text_and_the_use_since_its_start() {
        let base = Tokens { input: 100, output: 10, cache_read: 0, cache_write: 0 };
        let mut acc = TurnAcc::new("t1", base);
        assert!(acc.feed(&usage(150, 40, 30)).is_none());
        assert!(acc.feed(&agent_message(r#"{"ok":true}"#)).is_none());
        // Several requests in one turn: the total keeps growing, only the latest counts.
        assert!(acc.feed(&usage(300, 120, 90)).is_none());
        let out = acc.feed(&completed("completed")).unwrap().unwrap();
        assert_eq!(out.structured, Some(json!({"ok": true})));
        assert_eq!(out.text, r#"{"ok":true}"#);
        // total 300 in with 120 cached = 180 new; base was 100 new.
        assert_eq!(out.tokens, Tokens { input: 80, output: 80, cache_read: 120, cache_write: 0 });
        assert_eq!(acc.total().output, 90);
    }

    #[test]
    fn other_threads_are_ignored() {
        let mut acc = TurnAcc::new("other", Tokens::default());
        assert!(acc.feed(&completed("completed")).is_none());
        assert!(acc.feed(&agent_message("x")).is_none());
    }

    #[test]
    fn an_unfinished_turn_is_an_error() {
        let mut acc = TurnAcc::new("t1", Tokens::default());
        let e = acc.feed(&completed("interrupted")).unwrap().unwrap_err();
        assert!(e.contains("interrupted"), "{e}");
    }

    #[test]
    fn a_final_error_ends_the_turn_but_a_retry_does_not() {
        let mut acc = TurnAcc::new("t1", Tokens::default());
        let retry = note("error", json!({"threadId": "t1", "turnId": "u1", "willRetry": true, "error": {"message": "x"}}));
        assert!(acc.feed(&retry).is_none());
        let fatal = note("error", json!({"threadId": "t1", "turnId": "u1", "willRetry": false, "error": {"message": "quota"}}));
        assert_eq!(acc.feed(&fatal), Some(Err("quota".into())));
    }

    #[test]
    fn text_that_is_not_json_gives_no_structured_answer() {
        let mut acc = TurnAcc::new("t1", Tokens::default());
        acc.feed(&agent_message("sorry, I could not"));
        let out = acc.feed(&completed("completed")).unwrap().unwrap();
        assert_eq!(out.structured, None);
        assert_eq!(out.text, "sorry, I could not");
    }

    #[test]
    fn mcp_server_names_come_from_the_config_read() {
        let r = json!({"config": {"model": "x", "mcp_servers": {"homelab": {"command": "a"}, "node_repl": {}}}});
        let mut names = mcp_names(&r);
        names.sort();
        assert_eq!(names, vec!["homelab", "node_repl"]);
        assert!(mcp_names(&json!({"config": {"model": "x"}})).is_empty());
        assert!(mcp_names(&json!({})).is_empty());
    }

    #[test]
    fn the_transcript_names_what_the_agent_did() {
        let done = |item: Value| note("item/completed", json!({"threadId": "t1", "turnId": "u", "item": item}));
        assert_eq!(
            describe(&done(json!({"type": "commandExecution", "command": "cargo test"}))),
            Some("→ ran cargo test".into())
        );
        assert_eq!(describe(&done(json!({"type": "fileChange"}))), Some("→ changed files".into()));
        assert_eq!(describe(&done(json!({"type": "agentMessage", "text": "{}"}))), None);
        assert_eq!(describe(&json!({"method": "turn/started", "params": {}})), None);
    }

    #[test]
    fn requests_have_the_shape_the_schema_asks_for() {
        let spec = Spec {
            model: "gpt-6-luna".into(),
            system_prompt: "You search.".into(),
            read_only: true,
            cwd: PathBuf::from("/work"), proxy: None, web: None,
        };
        let t = thread_start_params(&spec, &[]);
        assert_eq!(t["sandbox"], "read-only");
        assert_eq!(t["approvalPolicy"], "never");
        assert_eq!(t["developerInstructions"], "You search.");
        assert_eq!(t["cwd"], "/work");
        assert_eq!(t["config"]["features"]["plugins"], false);

        let mut b = spec.clone();
        b.read_only = false;
        assert_eq!(thread_start_params(&b, &[])["sandbox"], "workspace-write");
        // Only an agent that changes files gets the network; a reader's sandbox stays closed.
        assert_eq!(thread_start_params(&b, &[])["config"]["sandbox_workspace_write"]["network_access"], true);
        assert!(t["config"].get("sandbox_workspace_write").is_none());

        // The user's own MCP servers are switched off by name, and only when there are some.
        assert!(t["config"].get("mcp_servers").is_none());
        let off = thread_start_params(&spec, &["homelab".to_string(), "node_repl".to_string()]);
        assert_eq!(off["config"]["mcp_servers"], json!({"homelab": {"enabled": false}, "node_repl": {"enabled": false}}));
        assert_eq!(off["config"]["features"]["plugins"], false, "the other switches stay");
        for f in ["computer_use", "browser_use", "multi_agent"] {
            assert_eq!(t["config"]["features"][f], false, "{f} must be off in an unattended agent");
        }

        let s = json!({"type": "object"});
        let turn = turn_start_params("t1", "go", &s);
        assert_eq!(turn["input"][0], json!({"type": "text", "text": "go"}));
        assert_eq!(turn["outputSchema"], s);
        assert_eq!(initialize_params()["clientInfo"]["name"], "ttm-orch");
    }

    /// A fresh empty folder for a live test to work in, removed when the test ends, so nothing lingers in the
    /// shared temp folder.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let d = std::env::temp_dir().join(format!("ttm-orch-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&d).unwrap();
            Scratch(d)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Makes one small real turn, so it uses a little of the plan: `cargo test -- --ignored codex_turn`.
    #[tokio::test]
    #[ignore]
    async fn codex_turn_returns_structured_output_and_usage() {
        let server = Server::start().await.unwrap();
        let scratch = Scratch::new("codex");
        let spec = Spec {
            model: "gpt-6-luna".into(),
            system_prompt: "Answer tersely.".into(),
            read_only: true,
            cwd: scratch.0.clone(), proxy: None, web: None,
        };
        let mut thread = server.start_thread(&spec).await.unwrap();
        let schema = json!({
            "type": "object", "properties": {"n": {"type": "integer"}},
            "required": ["n"], "additionalProperties": false
        });
        let mut seen = Vec::new();
        let out = server
            .turn(&mut thread, "Answer with n = 7. Do not run any commands.", &schema, |n| {
                seen.push(n["method"].as_str().unwrap_or("").to_string())
            })
            .await
            .unwrap();
        eprintln!("notifications: {seen:?}\noutcome: {out:?}");
        assert_eq!(out.structured, Some(json!({"n": 7})));
        assert!(out.tokens.total() > 0);
    }

    /// A builder's sandbox reaches the network and a reader's does not. Makes two small real turns:
    /// `cargo test -- --ignored codex_builder_network`.
    #[tokio::test]
    #[ignore]
    async fn codex_builder_network_is_open_and_a_readers_is_not() {
        let server = Server::start().await.unwrap();
        let schema = json!({
            "type": "object", "properties": {"code": {"type": "string"}},
            "required": ["code"], "additionalProperties": false
        });
        let ask = "Run this shell command: curl -sS -m 8 -o /dev/null -w '%{http_code}' https://registry.npmjs.org/left-pad ; echo. \
                   Put the number it printed in `code`, or the word FAILED if curl printed an error.";
        let mut codes = Vec::new();
        for read_only in [false, true] {
            let scratch = Scratch::new(if read_only { "net-ro" } else { "net-rw" });
            let spec = Spec { model: "gpt-6-luna".into(), system_prompt: "Do exactly what is asked.".into(), read_only, cwd: scratch.0.clone(), proxy: None, web: None };
            let mut thread = server.start_thread(&spec).await.unwrap();
            let out = server.turn(&mut thread, ask, &schema, |_| {}).await.unwrap();
            codes.push(out.structured.unwrap()["code"].as_str().unwrap().to_string());
        }
        eprintln!("builder: {}, reader: {}", codes[0], codes[1]);
        assert_eq!(codes[0], "200");
        assert_ne!(codes[1], "200");
    }

    /// Talks to the real `codex` but makes no model call: `cargo test -- --ignored codex_lists`.
    #[tokio::test]
    #[ignore]
    async fn codex_lists_the_models_this_account_can_use() {
        let server = Server::start().await.unwrap();
        let models = server.models().await.unwrap();
        eprintln!("models: {models:?}");
        assert!(models.iter().any(|m| m.starts_with("gpt-")), "{models:?}");
    }
}
