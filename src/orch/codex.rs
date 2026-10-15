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

use super::Outcome;
use super::usage::Tokens;

#[derive(Debug, Clone)]
pub struct Spec {
    pub model: String,
    pub system_prompt: String,
    pub read_only: bool,
    pub cwd: PathBuf,
}

pub fn initialize_params() -> Value {
    json!({"clientInfo": {"name": "ttm-orch", "title": "ttm", "version": env!("CARGO_PKG_VERSION")}})
}

pub fn thread_start_params(spec: &Spec) -> Value {
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
        "config": {"features": {"plugins": false, "hooks": false, "apps": false}},
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
                    is_error: false,
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

        let (p, n, w) = (pending.clone(), notes.clone(), stdin.clone());
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
            // The process ended: fail everything still waiting.
            for (_, tx) in p.lock().await.drain() {
                let _ = tx.send(Err("codex exited".into()));
            }
        });

        let server = Server { _child: child, stdin, pending, next: AtomicU64::new(1), notes };
        server.request("initialize", initialize_params()).await?;
        server.send(&json!({"method": "initialized"})).await?;
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
        let r = self.request("thread/start", thread_start_params(spec)).await?;
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

/// One shared server, started the first time a Codex node needs it.
#[derive(Default)]
pub struct Hub(tokio::sync::OnceCell<Server>);

impl Hub {
    pub async fn get(&self) -> Result<&Server> {
        self.0.get_or_try_init(Server::start).await
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
            cwd: PathBuf::from("/work"),
        };
        let t = thread_start_params(&spec);
        assert_eq!(t["sandbox"], "read-only");
        assert_eq!(t["approvalPolicy"], "never");
        assert_eq!(t["developerInstructions"], "You search.");
        assert_eq!(t["cwd"], "/work");
        assert_eq!(t["config"]["features"]["plugins"], false);

        let mut b = spec.clone();
        b.read_only = false;
        assert_eq!(thread_start_params(&b)["sandbox"], "workspace-write");

        let s = json!({"type": "object"});
        let turn = turn_start_params("t1", "go", &s);
        assert_eq!(turn["input"][0], json!({"type": "text", "text": "go"}));
        assert_eq!(turn["outputSchema"], s);
        assert_eq!(initialize_params()["clientInfo"]["name"], "ttm-orch");
    }

    /// A fresh empty folder for a live test to work in, so nothing lands in the shared temp folder.
    fn scratch_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ttm-orch-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Makes one small real turn, so it uses a little of the plan: `cargo test -- --ignored codex_turn`.
    #[tokio::test]
    #[ignore]
    async fn codex_turn_returns_structured_output_and_usage() {
        let server = Server::start().await.unwrap();
        let spec = Spec {
            model: "gpt-6-luna".into(),
            system_prompt: "Answer tersely.".into(),
            read_only: true,
            cwd: scratch_dir("codex"),
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
