//! The chat view's data: a Claude Code or Codex conversation log turned into chat items.
//!
//! Both agents append every turn to a JSON-lines file, which ttm already locates for the token
//! counts (`agentlog`). Reading it is cheap and works whatever drives the session (the browser,
//! the terminal view or a plain SSH attach). The log is parsed incrementally into a bounded list
//! of items. Each item carries a sequence number that rises whenever it is added or changed (a
//! tool call changes when its result arrives), so a client asks only for what changed since the
//! last number it saw.
//!
//! Claude writes one content block per line, so a block is one item and nothing is regrouped.
//! Codex writes `item_completed` events, one per finished item.

use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::{AppState, agentlog, config::Agent, modes, screen, sessions, usage};

/// Items kept per session; older ones are dropped (the client keeps what it already has).
const MAX_ITEMS: usize = 2000;
/// Longest tool input or output sent to the browser.
const PREVIEW_CHARS: usize = 2000;
/// Longest side of an edit shown as a diff.
const DIFF_CHARS: usize = 8000;
/// Longest single message.
const TEXT_CHARS: usize = 20_000;
/// A log bigger than this is read from near its end on first load.
const INITIAL_TAIL: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionState {
    /// The agent is waiting for an answer.
    Pending,
    Answered,
    /// Dismissed or interrupted without an answer.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditPair {
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diff {
    pub path: String,
    pub edits: Vec<EditPair>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Question {
    pub header: String,
    pub question: String,
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub question: String,
    pub answer: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    User { text: String, images: u32 },
    Assistant { text: String },
    Thinking { text: String },
    /// A slash command you ran, such as `/compact`.
    Command { text: String },
    /// Output of a local command, or a note such as "conversation compacted".
    Notice { text: String },
    Tool {
        tool: String,
        summary: String,
        input: Option<String>,
        output: Option<String>,
        status: ToolStatus,
        diff: Option<Diff>,
    },
    Question { questions: Vec<Question>, state: QuestionState, answers: Vec<Answer> },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item {
    pub id: String,
    /// Rises whenever the item is added or changed.
    pub seq: u64,
    /// Unix seconds, when the log says.
    pub ts: Option<i64>,
    #[serde(flatten)]
    pub body: Body,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned lock only means another thread panicked mid-update; the items are still usable.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------ helpers

/// A command without the shell Codex runs it through: `/bin/zsh -lc eza -1 src` is `eza -1 src`.
fn bare_command(cmd: &str) -> &str {
    let cmd = cmd.trim();
    let mut parts = cmd.splitn(3, ' ');
    let (shell, flag, rest) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next());
    let is_shell = matches!(shell.rsplit('/').next(), Some("sh" | "bash" | "zsh" | "fish" | "dash"));
    match rest {
        Some(rest) if is_shell && matches!(flag, "-c" | "-lc" | "-ic" | "-l" | "-i") => {
            let rest = rest.trim();
            // The script may arrive quoted as one argument.
            for q in ['\'', '"'] {
                if let Some(inner) = rest.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
                    return inner;
                }
            }
            rest
        }
        _ => cmd,
    }
}

/// `s` cut to `max` characters, with an ellipsis when it was longer.
fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

fn first_line(s: &str) -> &str {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim()
}

/// The text of a message `content`: a string, or an array of blocks that carry `text`.
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str).or_else(|| b.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The text between `open` and `close`, if both are present.
fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let end = s[start..].find(close)? + start;
    Some(&s[start..end])
}

/// `s` without any `<tag>...</tag>` sections.
fn strip_tagged(s: &str, tag: &str) -> String {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        match rest[start..].find(&close) {
            Some(end) => rest = &rest[start + end + close.len()..],
            None => return out, // unterminated: drop the tail
        }
    }
    out.push_str(rest);
    out
}

/// The last two path components, enough to recognise a file at a glance.
fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.trim_end_matches('/').rsplit('/').take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Microseconds since the epoch: unique enough for an epoch, and small enough (about 1.8e15) for a
/// JavaScript number to hold exactly, which nanoseconds are not.
fn now_micros() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64)
}

// -------------------------------------------------------------------- state

/// One conversation's parsed log.
pub struct ChatState {
    epoch: u64,
    seq: u64,
    agent: Agent,
    conv_id: String,
    path: Option<PathBuf>,
    offset: u64,
    items: Vec<Item>,
    index: HashMap<String, usize>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// A model name from a `/model` confirmation, such as `Haiku 5.5`, until a reply reports the id.
    pub model_label: Option<String>,
}

impl ChatState {
    pub fn new(agent: Agent, conv_id: &str) -> Self {
        Self {
            epoch: now_micros(),
            seq: 0,
            agent,
            conv_id: conv_id.to_owned(),
            path: None,
            offset: 0,
            items: Vec::new(),
            index: HashMap::new(),
            model: None,
            effort: None,
            model_label: None,
        }
    }

    fn reset(&mut self) {
        let (agent, id, old_epoch) = (self.agent, std::mem::take(&mut self.conv_id), self.epoch);
        *self = Self::new(agent, &id);
        self.epoch = self.epoch.max(old_epoch + 1); // always different from the one clients hold
    }

    fn add(&mut self, id: String, ts: Option<i64>, body: Body) {
        self.seq += 1;
        let item = Item { id: id.clone(), seq: self.seq, ts, body };
        if let Some(&pos) = self.index.get(&id) {
            self.items[pos] = item;
            return;
        }
        self.index.insert(id, self.items.len());
        self.items.push(item);
        if self.items.len() > MAX_ITEMS {
            let extra = self.items.len() - MAX_ITEMS;
            self.items.drain(..extra);
            self.index = self.items.iter().enumerate().map(|(i, it)| (it.id.clone(), i)).collect();
        }
    }

    /// Change an existing item in place. Returns false when it is not (or no longer) there.
    fn modify(&mut self, id: &str, change: impl FnOnce(&mut Body)) -> bool {
        let Some(&pos) = self.index.get(id) else { return false };
        self.seq += 1;
        self.items[pos].seq = self.seq;
        change(&mut self.items[pos].body);
        true
    }

    /// Read what was appended to the log since last time. Returns whether a log exists yet.
    /// Blocking: reads the file.
    pub fn update(&mut self) -> std::io::Result<bool> {
        if self.path.is_none() {
            self.path = agentlog::find_log(self.agent, &self.conv_id);
        }
        let Some(path) = self.path.clone() else { return Ok(false) };
        let mut f = File::open(&path)?;
        let len = f.metadata()?.len();
        if len < self.offset {
            self.reset();
            self.path = Some(path);
        }
        if len == self.offset {
            return Ok(true);
        }
        let mut skip_partial = false;
        if self.offset == 0 && len > INITIAL_TAIL {
            self.offset = len - INITIAL_TAIL;
            skip_partial = true; // we landed mid-line
        }
        f.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        f.take(len - self.offset).read_to_end(&mut buf)?;
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return Ok(true) };
        let mut lines = buf[..end].split(|&b| b == b'\n');
        if skip_partial {
            lines.next();
        }
        for line in lines {
            self.ingest(line);
        }
        self.offset += end as u64 + 1;
        Ok(true)
    }

    /// Items added or changed after `after`, oldest first.
    fn since(&self, after: u64) -> Vec<Item> {
        self.items.iter().filter(|i| i.seq > after).cloned().collect()
    }

    pub fn ingest(&mut self, line: &[u8]) {
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
        match self.agent {
            Agent::Claude => self.ingest_claude(&v),
            Agent::Codex => self.ingest_codex(&v),
            Agent::Other => {}
        }
    }

    // ------------------------------------------------------------- Claude

    fn ingest_claude(&mut self, v: &Value) {
        if v["isSidechain"].as_bool() == Some(true) {
            return; // a sub-agent's own conversation
        }
        let ts = v.get("timestamp").and_then(usage::parse_time);
        let uuid = str_of(v, "uuid").to_owned();
        match v["type"].as_str() {
            Some("assistant") => self.claude_assistant(v, &uuid, ts),
            Some("user") => self.claude_user(v, &uuid, ts),
            _ => {}
        }
    }

    fn claude_assistant(&mut self, v: &Value, uuid: &str, ts: Option<i64>) {
        let msg = &v["message"];
        if let Some(m) = msg["model"].as_str().filter(|m| !m.is_empty() && !m.starts_with('<')) {
            self.model = Some(m.to_owned());
            self.model_label = None; // the reply itself says which model it was
        }
        if let Some(e) = v["effort"].as_str().filter(|e| !e.is_empty()) {
            self.effort = Some(e.to_owned());
        }
        let Some(blocks) = msg["content"].as_array() else { return };
        for (i, b) in blocks.iter().enumerate() {
            let id = format!("{uuid}:{i}");
            match b["type"].as_str() {
                Some("text") => {
                    let t = str_of(b, "text").trim();
                    if !t.is_empty() {
                        self.add(id, ts, Body::Assistant { text: clip(t, TEXT_CHARS) });
                    }
                }
                Some("thinking") => {
                    let t = str_of(b, "thinking").trim();
                    if !t.is_empty() {
                        self.add(id, ts, Body::Thinking { text: clip(t, TEXT_CHARS) });
                    }
                }
                Some("tool_use") => self.claude_tool_use(b, ts),
                _ => {}
            }
        }
    }

    fn claude_tool_use(&mut self, b: &Value, ts: Option<i64>) {
        let (id, name, input) = (str_of(b, "id").to_owned(), str_of(b, "name"), &b["input"]);
        if id.is_empty() {
            return;
        }
        if name == "AskUserQuestion" {
            let questions = parse_questions(input);
            if !questions.is_empty() {
                self.add(id, ts, Body::Question { questions, state: QuestionState::Pending, answers: Vec::new() });
                return;
            }
        }
        let (summary, shown_input, diff) = claude_tool_summary(name, input);
        self.add(
            id,
            ts,
            Body::Tool { tool: name.to_owned(), summary, input: shown_input, output: None, status: ToolStatus::Running, diff },
        );
    }

    fn claude_user(&mut self, v: &Value, uuid: &str, ts: Option<i64>) {
        if v["isMeta"].as_bool() == Some(true) {
            return;
        }
        if v["isCompactSummary"].as_bool() == Some(true) {
            self.add(uuid.to_owned(), ts, Body::Notice { text: "Conversation compacted".into() });
            return;
        }
        let content = &v["message"]["content"];
        let Some(blocks) = content.as_array() else {
            if let Some(t) = content.as_str() {
                self.user_text(uuid.to_owned(), ts, t, 0);
            }
            return;
        };
        let mut texts: Vec<&str> = Vec::new();
        let mut images = 0;
        for b in blocks {
            match b["type"].as_str() {
                Some("text") => texts.push(str_of(b, "text")),
                Some("image") => images += 1,
                Some("tool_result") => self.claude_tool_result(b, v),
                _ => {}
            }
        }
        if !texts.is_empty() || images > 0 {
            self.user_text(uuid.to_owned(), ts, &texts.join("\n"), images);
        }
    }

    fn claude_tool_result(&mut self, b: &Value, line: &Value) {
        let id = str_of(b, "tool_use_id");
        let is_error = b["is_error"].as_bool() == Some(true);
        let output = text_of(&b["content"]);
        // The question tool reports the answers in a structured form.
        let answers: Vec<Answer> = line["toolUseResult"]["answers"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(q, a)| Answer { question: q.clone(), answer: a.as_str().unwrap_or("").to_owned() })
                    .collect()
            })
            .unwrap_or_default();
        self.modify(id, |body| match body {
            Body::Tool { output: o, status, .. } => {
                *o = Some(clip(output.trim(), PREVIEW_CHARS)).filter(|s| !s.is_empty());
                *status = if is_error { ToolStatus::Error } else { ToolStatus::Ok };
            }
            Body::Question { state, answers: a, .. } => {
                if answers.is_empty() {
                    *state = QuestionState::Cancelled;
                } else {
                    *state = QuestionState::Answered;
                    *a = answers;
                }
            }
            _ => {}
        });
    }

    /// A message you typed, or one of the wrappers Claude puts around slash commands.
    fn user_text(&mut self, id: String, ts: Option<i64>, text: &str, images: u32) {
        let cleaned = strip_tagged(text, "system-reminder");
        let t = cleaned.trim();
        if let Some(name) = between(t, "<command-name>", "</command-name>") {
            let args = between(t, "<command-args>", "</command-args>").unwrap_or("").trim();
            let text = format!("{} {args}", name.trim()).trim().to_owned();
            self.add(id, ts, Body::Command { text });
            return;
        }
        if let Some(out) = between(t, "<local-command-stdout>", "</local-command-stdout>") {
            self.note_switch(out);
            if !out.trim().is_empty() {
                self.add(id, ts, Body::Notice { text: clip(out.trim(), PREVIEW_CHARS) });
            }
            return;
        }
        if t.starts_with("<local-command-") || t.starts_with("<command-message>") {
            return; // the "caveat" Claude adds after local commands
        }
        // Claude records an interruption as a line from "you"; it is a note, not something you typed.
        if t.starts_with("[Request interrupted by user") {
            self.add(id, ts, Body::Notice { text: "Interrupted".into() });
            return;
        }
        // Claude describes an attached image in a text block of its own.
        let t = t.lines().filter(|l| !(l.starts_with("[Image") && l.ends_with(']'))).collect::<Vec<_>>().join("\n");
        let t = t.trim();
        if t.is_empty() && images == 0 {
            return;
        }
        self.add(id, ts, Body::User { text: clip(t, TEXT_CHARS), images });
    }

    /// `/model` and `/effort` confirm themselves in the log: take the new setting from that line,
    /// so the chip is right straight away, whether it was changed here or in the terminal.
    fn note_switch(&mut self, out: &str) {
        if let Some(rest) = out.trim().strip_prefix("Set model to ") {
            let name = rest.split(" for this session").next().unwrap_or(rest);
            let name = name.trim().trim_matches('`').trim();
            if !name.is_empty() {
                self.model_label = Some(name.to_owned());
            }
        } else if let Some(rest) = out.trim().strip_prefix("Set effort level to ") {
            let level = rest.split([' ', ':', '(']).next().unwrap_or("");
            if !level.is_empty() {
                self.effort = Some(level.to_owned());
            }
        }
    }

    // -------------------------------------------------------------- Codex

    fn ingest_codex(&mut self, v: &Value) {
        let ts = v.get("timestamp").and_then(usage::parse_time);
        let payload = &v["payload"];
        match (v["type"].as_str(), payload["type"].as_str()) {
            (Some("turn_context"), _) => {
                if let Some(m) = payload["model"].as_str() {
                    self.model = Some(m.to_owned());
                }
                if let Some(e) = payload["effort"].as_str() {
                    self.effort = Some(e.to_owned());
                }
            }
            (Some("event_msg"), Some("item_completed")) => self.codex_item(&payload["item"], ts),
            _ => {}
        }
    }

    fn codex_item(&mut self, item: &Value, ts: Option<i64>) {
        let id = str_of(item, "id").to_owned();
        if id.is_empty() {
            return;
        }
        match item["type"].as_str() {
            Some("UserMessage") => {
                let t = text_of(&item["content"]);
                if !t.trim().is_empty() {
                    self.add(id, ts, Body::User { text: clip(t.trim(), TEXT_CHARS), images: 0 });
                }
            }
            Some("AgentMessage") => {
                let t = text_of(&item["content"]);
                if !t.trim().is_empty() {
                    self.add(id, ts, Body::Assistant { text: clip(t.trim(), TEXT_CHARS) });
                }
            }
            Some("Reasoning") => {
                let t = text_of(&item["summary_text"]);
                let t = if t.trim().is_empty() { text_of(&item["raw_content"]) } else { t };
                if !t.trim().is_empty() {
                    self.add(id, ts, Body::Thinking { text: clip(t.trim(), TEXT_CHARS) });
                }
            }
            Some("CommandExecution") => {
                let command = match &item["command"] {
                    Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
                    v => v.as_str().unwrap_or("").to_owned(),
                };
                let output = ["aggregated_output", "formatted_output", "stdout"]
                    .iter()
                    .map(|k| str_of(item, k))
                    .find(|s| !s.trim().is_empty())
                    .map(|s| clip(s.trim(), PREVIEW_CHARS));
                let command = bare_command(&command).to_owned();
                let failed = item["exit_code"].as_i64().is_some_and(|c| c != 0)
                    || matches!(item["status"].as_str(), Some("Failed" | "failed" | "Declined" | "declined"));
                self.add(
                    id,
                    ts,
                    Body::Tool {
                        tool: "Bash".into(),
                        summary: clip(first_line(&command), 120),
                        input: Some(clip(&command, PREVIEW_CHARS)),
                        output,
                        status: if failed { ToolStatus::Error } else { ToolStatus::Ok },
                        diff: None,
                    },
                );
            }
            Some(other) => {
                // Anything else (patches, MCP calls, sub-agents): show that it happened.
                let detail = item
                    .as_object()
                    .map(|o| {
                        o.iter()
                            .filter(|(k, _)| !matches!(k.as_str(), "id" | "type"))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<serde_json::Map<_, _>>()
                    })
                    .map(|m| clip(&Value::Object(m).to_string(), PREVIEW_CHARS));
                self.add(
                    id,
                    ts,
                    Body::Tool {
                        tool: other.to_owned(),
                        summary: other.to_owned(),
                        input: detail,
                        output: None,
                        status: ToolStatus::Ok,
                        diff: None,
                    },
                );
            }
            None => {}
        }
    }
}

fn parse_questions(input: &Value) -> Vec<Question> {
    input["questions"]
        .as_array()
        .map(|qs| {
            qs.iter()
                .map(|q| Question {
                    header: str_of(q, "header").to_owned(),
                    question: str_of(q, "question").to_owned(),
                    multi_select: q["multiSelect"].as_bool() == Some(true),
                    options: q["options"]
                        .as_array()
                        .map(|os| {
                            os.iter()
                                .map(|o| QuestionOption {
                                    label: str_of(o, "label").to_owned(),
                                    description: str_of(o, "description").to_owned(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One line saying what a Claude tool call did, what it was given, and any file change.
fn claude_tool_summary(name: &str, input: &Value) -> (String, Option<String>, Option<Diff>) {
    let path = str_of(input, "file_path");
    let edit = |old: &str, new: &str| EditPair { old: clip(old, DIFF_CHARS), new: clip(new, DIFF_CHARS) };
    match name {
        "Bash" => {
            let cmd = str_of(input, "command");
            let said = str_of(input, "description");
            let summary = if said.is_empty() { first_line(cmd) } else { said };
            (clip(summary, 120), Some(clip(cmd, PREVIEW_CHARS)), None)
        }
        "Read" => (format!("Read {}", short_path(path)), None, None),
        "Edit" => (
            format!("Edited {}", short_path(path)),
            None,
            Some(Diff { path: path.to_owned(), edits: vec![edit(str_of(input, "old_string"), str_of(input, "new_string"))] }),
        ),
        "MultiEdit" => {
            let edits = input["edits"]
                .as_array()
                .map(|es| es.iter().map(|e| edit(str_of(e, "old_string"), str_of(e, "new_string"))).collect())
                .unwrap_or_default();
            (format!("Edited {}", short_path(path)), None, Some(Diff { path: path.to_owned(), edits }))
        }
        "Write" => (
            format!("Wrote {}", short_path(path)),
            None,
            Some(Diff { path: path.to_owned(), edits: vec![edit("", str_of(input, "content"))] }),
        ),
        "Glob" => (format!("Found files matching {}", str_of(input, "pattern")), None, None),
        "Grep" => (format!("Searched for {}", clip(str_of(input, "pattern"), 80)), None, None),
        "WebFetch" => (format!("Fetched {}", str_of(input, "url")), None, None),
        "WebSearch" => (format!("Searched the web for {}", str_of(input, "query")), None, None),
        "Task" | "Agent" => (clip(str_of(input, "description"), 120), Some(clip(str_of(input, "prompt"), PREVIEW_CHARS)), None),
        "TodoWrite" => ("Updated the to-do list".into(), None, None),
        "Skill" => (format!("Skill {}", str_of(input, "skill")), None, None),
        _ => (name.to_owned(), Some(clip(&input.to_string(), PREVIEW_CHARS)), None),
    }
}

// ----------------------------------------------------------------- the cache

/// Parsed logs, one per session.
#[derive(Clone, Default)]
pub struct ChatCache {
    states: Arc<Mutex<HashMap<String, ChatState>>>,
}

#[derive(Serialize)]
pub struct ChatResponse {
    /// Identifies this parse; a client holding a different epoch must start over.
    pub epoch: u64,
    /// The newest sequence number; send it back as `after`.
    pub seq: u64,
    /// The client's copy is unusable: replace it with `items`.
    pub reset: bool,
    pub items: Vec<Item>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// A model name from a `/model` confirmation that no reply has reported yet.
    pub model_label: Option<String>,
    /// The session is a Claude or Codex one whose conversation id is known, so a chat view makes
    /// sense even before the log file exists.
    pub eligible: bool,
    /// Which agent runs in the session, when it is a Claude or Codex one.
    pub agent: Option<Agent>,
    /// A conversation log exists. False before the agent has said anything.
    pub ready: bool,
    /// What the screen shows right now: working, waiting, or a question to answer.
    pub screen: Option<screen::Analysis>,
    /// The agent's plan/build mode, read from its footer.
    pub mode: Option<modes::Mode>,
    /// The modes this session can be switched between.
    pub modes: Vec<modes::ModeInfo>,
    /// Started with permission checks skipped.
    pub yolo: bool,
    /// The reasoning level on the agent's screen right now. Newer than `effort`, which comes from
    /// the log and only changes with the agent's next reply.
    pub effort_live: Option<String>,
    /// The greyed-out reply Claude suggests in its empty input box.
    pub suggestion: Option<String>,
}

impl ChatCache {
    /// The questions the agent is waiting on in `session`, after reading any new log lines.
    /// Blocking: reads the log. Call from `spawn_blocking`.
    pub fn pending_question(&self, session: &str) -> Option<Vec<Question>> {
        let mut states = lock(&self.states);
        let state = states.get_mut(session)?;
        state.update().ok()?;
        state.items.iter().rev().find_map(|i| match &i.body {
            Body::Question { questions, state: QuestionState::Pending, .. } => Some(questions.clone()),
            _ => None,
        })
    }

    /// Blocking: reads the log. Call from `spawn_blocking`.
    fn read(&self, session: &str, agent: Agent, conv_id: &str, epoch: Option<u64>, after: u64) -> (ChatResponse, bool) {
        let mut states = lock(&self.states);
        let stale = states.get(session).is_none_or(|s| s.agent != agent || s.conv_id != conv_id);
        if stale {
            states.insert(session.to_owned(), ChatState::new(agent, conv_id));
        }
        let Some(state) = states.get_mut(session) else { unreachable!("inserted above") };
        let ready = state.update().unwrap_or_else(|e| {
            tracing::warn!("reading the {} log for {session}: {e}", conv_id);
            false
        });
        let reset = epoch != Some(state.epoch);
        let items = if reset { state.since(0) } else { state.since(after) };
        let response = ChatResponse {
            epoch: state.epoch,
            seq: state.seq,
            reset,
            items,
            model: state.model.clone(),
            effort: state.effort.clone(),
            model_label: state.model_label.clone(),
            eligible: true,
            agent: Some(agent),
            ready,
            screen: None,
            mode: None,
            modes: Vec::new(),
            yolo: false,
            effort_live: None,
            suggestion: None,
        };
        (response, ready)
    }
}

/// What changed in `session`'s conversation since `(epoch, after)`.
pub async fn snapshot(st: &Arc<AppState>, session: &str, epoch: Option<u64>, after: u64) -> Result<ChatResponse> {
    let (agent, conv_id) = st
        .sessions
        .get(session)
        .and_then(|m| Some((m.agent()?, m.agent_session_id.clone()?)))
        .filter(|(a, id)| *a != Agent::Other && sessions::valid_agent_id(id))
        .map_or((None, None), |(a, id)| (Some(a), Some(id)));
    // One capture of the pane feeds both the question reader and the mode footer.
    let text = crate::tmux::capture(session, 80).await.ok();
    let mut screen = text.as_deref().map(screen::analyze);
    // The model and effort menus look like questions to the screen reader, but they are driven by
    // the picker sheet: offering their rows as answer buttons would be wrong (and is refused).
    if let (Some(a), Some(t)) = (screen.as_mut(), text.as_deref())
        && a.prompt.is_some()
        && crate::modelmenu::picker_open(t)
    {
        a.prompt = None;
    }
    let (Some(agent), Some(conv_id)) = (agent, conv_id) else {
        return Ok(ChatResponse {
            epoch: 0,
            seq: 0,
            reset: epoch != Some(0),
            items: Vec::new(),
            model: None,
            effort: None,
            model_label: None,
            eligible: false,
            agent: None,
            ready: false,
            screen,
            mode: None,
            modes: Vec::new(),
            yolo: false,
            effort_live: None,
            suggestion: None,
        });
    };
    let cache = st.chat.clone();
    let name = session.to_owned();
    let (mut response, _) = tokio::task::spawn_blocking(move || cache.read(&name, agent, &conv_id, epoch, after)).await?;
    response.screen = screen;
    response.yolo = st.sessions.get(session).is_some_and(|m| m.spec.yolo);
    response.mode = text.as_deref().and_then(|t| modes::detect(agent, t));
    response.modes = modes::available(agent, response.yolo, response.mode);
    // The screen is the freshest source; Claude's status line (which hides the on-screen
    // indicator) is the fallback.
    let uid = st.sessions.get(session).map(|m| m.uid.clone()).unwrap_or_default();
    response.effort_live = text
        .as_deref()
        .and_then(|t| modes::live_effort(agent, t))
        .or_else(|| st.statusline.get(&uid).and_then(|s| s.effort.clone()));
    // Only worth a second capture when the input line shows something.
    if agent == Agent::Claude && text.as_deref().is_some_and(input_has_text) {
        response.suggestion = tmux_styled_suggestion(session).await;
    }
    Ok(response)
}

/// The last `❯` line in the plain screen has something after it.
fn input_has_text(screen: &str) -> bool {
    screen
        .lines()
        .rev()
        .take(14)
        .find_map(|l| l.trim_start().strip_prefix('❯'))
        .is_some_and(|rest| !rest.trim().is_empty())
}

async fn tmux_styled_suggestion(session: &str) -> Option<String> {
    let styled = crate::tmux::capture_styled(session).await.ok()?;
    crate::placeholder::faint_input(&styled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claude() -> ChatState {
        ChatState::new(Agent::Claude, "conv")
    }

    fn feed(s: &mut ChatState, lines: &[Value]) {
        for l in lines {
            s.ingest(l.to_string().as_bytes());
        }
    }

    fn bodies(s: &ChatState) -> Vec<&Body> {
        s.items.iter().map(|i| &i.body).collect()
    }

    fn assistant(uuid: &str, block: Value) -> Value {
        json!({ "type": "assistant", "uuid": uuid, "timestamp": "2026-10-08T22:00:00.000Z",
                "effort": "medium", "message": { "id": "m1", "model": "claude-sonnet-5-5", "content": [block] } })
    }

    fn user_text(uuid: &str, text: &str) -> Value {
        json!({ "type": "user", "uuid": uuid, "message": { "role": "user", "content": text } })
    }

    #[test]
    fn claude_text_and_thinking_blocks_each_become_an_item() {
        let mut s = claude();
        feed(
            &mut s,
            &[
                user_text("u1", "hello there"),
                assistant("a1", json!({ "type": "thinking", "thinking": "hmm" })),
                assistant("a2", json!({ "type": "text", "text": "Hi!" })),
            ],
        );
        assert_eq!(
            bodies(&s),
            vec![
                &Body::User { text: "hello there".into(), images: 0 },
                &Body::Thinking { text: "hmm".into() },
                &Body::Assistant { text: "Hi!".into() },
            ]
        );
        assert_eq!(s.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(s.effort.as_deref(), Some("medium"));
        assert_eq!(s.items[2].ts, Some(1_791_496_800));
    }

    #[test]
    fn tool_call_is_updated_in_place_when_its_result_arrives() {
        let mut s = claude();
        feed(
            &mut s,
            &[assistant(
                "a1",
                json!({ "type": "tool_use", "id": "t1", "name": "Bash",
                        "input": { "command": "cargo test\n--quiet", "description": "Run the tests" } }),
            )],
        );
        let before = s.seq;
        assert!(matches!(&s.items[0].body, Body::Tool { status: ToolStatus::Running, summary, .. } if summary == "Run the tests"));
        feed(
            &mut s,
            &[json!({ "type": "user", "uuid": "u2", "message": { "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": "61 passed" } ] } })],
        );
        assert_eq!(s.items.len(), 1, "the result must not add an item");
        assert!(s.items[0].seq > before, "the change must be visible to a client that saw the old one");
        let Body::Tool { status, output, input, .. } = &s.items[0].body else { panic!("not a tool") };
        assert_eq!((*status, output.as_deref(), input.as_deref()), (ToolStatus::Ok, Some("61 passed"), Some("cargo test\n--quiet")));
    }

    #[test]
    fn failed_and_interrupted_tools_show_as_errors() {
        let mut s = claude();
        feed(
            &mut s,
            &[
                assistant("a1", json!({ "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "false" } })),
                json!({ "type": "user", "uuid": "u2", "message": { "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "is_error": true,
                      "content": [{ "type": "text", "text": "exit 1" }] } ] } }),
            ],
        );
        assert!(matches!(&s.items[0].body, Body::Tool { status: ToolStatus::Error, output: Some(o), .. } if o == "exit 1"));
    }

    #[test]
    fn edits_carry_a_diff_and_a_short_path() {
        let mut s = claude();
        feed(
            &mut s,
            &[
                assistant("a1", json!({ "type": "tool_use", "id": "t1", "name": "Edit",
                    "input": { "file_path": "/p/src/api.rs", "old_string": "a", "new_string": "b" } })),
                assistant("a2", json!({ "type": "tool_use", "id": "t2", "name": "Write",
                    "input": { "file_path": "/p/new.txt", "content": "hi" } })),
            ],
        );
        let Body::Tool { summary, diff, .. } = &s.items[0].body else { panic!() };
        assert_eq!(summary, "Edited src/api.rs");
        assert_eq!(diff.as_ref().unwrap().edits, vec![EditPair { old: "a".into(), new: "b".into() }]);
        let Body::Tool { summary, diff, .. } = &s.items[1].body else { panic!() };
        assert_eq!(summary, "Wrote p/new.txt");
        assert_eq!(diff.as_ref().unwrap().edits[0], EditPair { old: String::new(), new: "hi".into() });
    }

    #[test]
    fn a_question_is_pending_until_answered() {
        let mut s = claude();
        feed(
            &mut s,
            &[assistant("a1", json!({ "type": "tool_use", "id": "q1", "name": "AskUserQuestion", "input": { "questions": [
                { "header": "Colour", "question": "Which?", "multiSelect": false,
                  "options": [{ "label": "Red", "description": "warm" }, { "label": "Blue", "description": "cool" }] } ] } }))],
        );
        let Body::Question { questions, state, .. } = &s.items[0].body else { panic!("not a question") };
        assert_eq!(*state, QuestionState::Pending);
        assert_eq!(questions[0].options[1].label, "Blue");
        feed(
            &mut s,
            &[json!({ "type": "user", "uuid": "u2", "toolUseResult": { "answers": { "Which?": "Blue" } },
                      "message": { "content": [{ "type": "tool_result", "tool_use_id": "q1", "content": "The user answered" }] } })],
        );
        let Body::Question { state, answers, .. } = &s.items[0].body else { panic!() };
        assert_eq!(*state, QuestionState::Answered);
        assert_eq!(answers, &vec![Answer { question: "Which?".into(), answer: "Blue".into() }]);
    }

    #[test]
    fn a_question_dismissed_without_answers_is_cancelled() {
        let mut s = claude();
        feed(
            &mut s,
            &[
                assistant("a1", json!({ "type": "tool_use", "id": "q1", "name": "AskUserQuestion", "input": { "questions": [
                    { "header": "H", "question": "Q?", "options": [] } ] } })),
                json!({ "type": "user", "uuid": "u2", "message": { "content": [
                    { "type": "tool_result", "tool_use_id": "q1", "is_error": true, "content": "dismissed" } ] } }),
            ],
        );
        assert!(matches!(&s.items[0].body, Body::Question { state: QuestionState::Cancelled, .. }));
    }

    #[test]
    fn internal_lines_and_wrappers_are_hidden_or_tidied() {
        let mut s = claude();
        let mut meta = user_text("u0", "internal");
        meta["isMeta"] = json!(true);
        let mut side = assistant("s1", json!({ "type": "text", "text": "from a sub-agent" }));
        side["isSidechain"] = json!(true);
        feed(
            &mut s,
            &[
                meta,
                side,
                user_text("u1", "<local-command-caveat>Caveat: ignore</local-command-caveat>"),
                user_text("u2", "<command-message>grill-me</command-message>\n<command-name>/grill-me</command-name>\n<command-args>the plan</command-args>"),
                user_text("u3", "<local-command-stdout>Set model to Sonnet 5 for this session only</local-command-stdout>"),
                user_text("u4", "<system-reminder>secret context</system-reminder>real question"),
                user_text("u5", "<system-reminder>only a reminder</system-reminder>"),
                json!({ "type": "attachment", "uuid": "x" }),
            ],
        );
        assert_eq!(
            bodies(&s),
            vec![
                &Body::Command { text: "/grill-me the plan".into() },
                &Body::Notice { text: "Set model to Sonnet 5 for this session only".into() },
                &Body::User { text: "real question".into(), images: 0 },
            ]
        );
    }

    #[test]
    fn model_and_effort_changes_are_read_from_their_confirmations() {
        let mut s = claude();
        feed(&mut s, &[assistant("a1", json!({ "type": "text", "text": "hi" }))]);
        assert_eq!((s.model.as_deref(), s.model_label.as_deref()), (Some("claude-sonnet-5-5"), None));
        feed(
            &mut s,
            &[
                user_text("u1", "<local-command-stdout>Set model to `Haiku 5.5` for this session only</local-command-stdout>"),
                user_text("u2", "<local-command-stdout>Set effort level to high (this session only): Comprehensive implementation</local-command-stdout>"),
            ],
        );
        assert_eq!(s.model_label.as_deref(), Some("Haiku 5.5"));
        assert_eq!(s.effort.as_deref(), Some("high"));
        // The next reply carries the real model id, which replaces the label.
        feed(&mut s, &[assistant("a2", json!({ "type": "text", "text": "ok" }))]);
        assert_eq!(s.model_label, None);
        // Unrelated command output changes nothing.
        feed(&mut s, &[user_text("u3", "<local-command-stdout>Total cost: $0.12</local-command-stdout>")]);
        assert_eq!(s.model_label, None);
    }

    #[test]
    fn an_interruption_is_a_notice_not_a_message_from_you() {
        let mut s = claude();
        feed(
            &mut s,
            &[
                user_text("u1", "[Request interrupted by user for tool use]"),
                user_text("u2", "[Request interrupted by user]"),
                user_text("u3", "a real message that mentions [Request interrupted by user]"),
            ],
        );
        assert_eq!(
            bodies(&s),
            vec![
                &Body::Notice { text: "Interrupted".into() },
                &Body::Notice { text: "Interrupted".into() },
                &Body::User { text: "a real message that mentions [Request interrupted by user]".into(), images: 0 },
            ]
        );
    }

    #[test]
    fn images_are_counted_and_their_description_lines_dropped() {
        let mut s = claude();
        feed(
            &mut s,
            &[json!({ "type": "user", "uuid": "u1", "message": { "content": [
                { "type": "image", "source": {} },
                { "type": "text", "text": "[Image: original 100x100, displayed at 100x100.]" },
                { "type": "text", "text": "what is this?" } ] } })],
        );
        assert_eq!(bodies(&s), vec![&Body::User { text: "what is this?".into(), images: 1 }]);
    }

    #[test]
    fn codex_items_map_to_chat_items() {
        let mut s = ChatState::new(Agent::Codex, "thread");
        let done = |item: Value| json!({ "type": "event_msg", "payload": { "type": "item_completed", "item": item } });
        feed(
            &mut s,
            &[
                json!({ "type": "turn_context", "payload": { "model": "gpt-6.1-sol", "effort": "medium" } }),
                done(json!({ "id": "1", "type": "UserMessage", "content": [{ "type": "text", "text": "run the tests" }] })),
                done(json!({ "id": "2", "type": "Reasoning", "summary_text": ["Planning"], "raw_content": [] })),
                done(json!({ "id": "3", "type": "CommandExecution", "command": "cargo test", "aggregated_output": "ok",
                             "exit_code": 0, "status": "Completed" })),
                done(json!({ "id": "4", "type": "CommandExecution", "command": ["ls", "nope"], "aggregated_output": "no such file",
                             "exit_code": 2 })),
                done(json!({ "id": "5", "type": "AgentMessage", "phase": "final_answer", "content": [{ "type": "text", "text": "Done." }] })),
                done(json!({ "id": "6", "type": "SubAgentActivity", "kind": "spawn" })),
                json!({ "type": "event_msg", "payload": { "type": "token_count" } }),
            ],
        );
        assert_eq!((s.model.as_deref(), s.effort.as_deref()), (Some("gpt-6.1-sol"), Some("medium")));
        let kinds: Vec<&str> = s
            .items
            .iter()
            .map(|i| match &i.body {
                Body::User { .. } => "user",
                Body::Thinking { .. } => "thinking",
                Body::Assistant { .. } => "assistant",
                Body::Tool { status: ToolStatus::Ok, .. } => "tool-ok",
                Body::Tool { status: ToolStatus::Error, .. } => "tool-err",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["user", "thinking", "tool-ok", "tool-err", "assistant", "tool-ok"]);
        let Body::Tool { input, .. } = &s.items[3].body else { panic!() };
        assert_eq!(input.as_deref(), Some("ls nope"));
    }

    #[test]
    fn a_client_gets_only_what_changed_since_its_last_sequence_number() {
        let mut s = claude();
        feed(&mut s, &[user_text("u1", "one"), user_text("u2", "two")]);
        let seen = s.seq;
        assert_eq!(s.since(seen).len(), 0);
        feed(&mut s, &[user_text("u3", "three")]);
        let new = s.since(seen);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].id, "u3");
        assert_eq!(s.since(0).len(), 3);
    }

    #[test]
    fn old_items_are_dropped_beyond_the_cap() {
        let mut s = claude();
        for i in 0..(MAX_ITEMS + 5) {
            feed(&mut s, &[user_text(&format!("u{i}"), &format!("m{i}"))]);
        }
        assert_eq!(s.items.len(), MAX_ITEMS);
        assert_eq!(s.items[0].id, "u5");
        // The index still finds items after the shift, so a late tool result still lands.
        assert!(s.modify("u10", |_| {}));
        assert!(!s.modify("u0", |_| {}));
    }

    #[test]
    fn reading_a_log_is_incremental_and_restarts_when_it_shrinks() {
        let dir = std::env::temp_dir().join(format!("ttm-chat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("log.jsonl");
        let line = |id: &str, text: &str| user_text(id, text).to_string();
        std::fs::write(&path, format!("{}\n{}", line("u1", "first"), line("u2", "second"))).unwrap(); // no final newline
        let mut s = claude();
        s.path = Some(path.clone());
        assert!(s.update().unwrap());
        assert_eq!(s.items.len(), 1, "a half-written last line must wait");
        let epoch = s.epoch;
        std::fs::write(&path, format!("{}\n{}\n{}\n", line("u1", "first"), line("u2", "second"), line("u3", "third"))).unwrap();
        s.update().unwrap();
        assert_eq!(s.items.len(), 3);
        assert_eq!(s.epoch, epoch, "appending must not reset clients");
        std::fs::write(&path, format!("{}\n", line("z", "fresh"))).unwrap();
        s.update().unwrap();
        assert_eq!(s.items.len(), 1);
        assert_ne!(s.epoch, epoch, "a rewritten log must make clients start over");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Run against a real log to see how it parses:
    /// `TTM_TEST_LOG=<file> TTM_TEST_AGENT=claude cargo test real_log -- --ignored --nocapture`
    #[test]
    #[ignore = "needs TTM_TEST_LOG and TTM_TEST_AGENT"]
    fn real_log() {
        let (Ok(path), Ok(agent)) = (std::env::var("TTM_TEST_LOG"), std::env::var("TTM_TEST_AGENT")) else { return };
        let agent = if agent == "codex" { Agent::Codex } else { Agent::Claude };
        let mut s = ChatState::new(agent, "real");
        s.path = Some(path.into());
        assert!(s.update().unwrap());
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        for i in &s.items {
            let kind = match &i.body {
                Body::User { .. } => "user".to_owned(),
                Body::Assistant { .. } => "assistant".into(),
                Body::Thinking { .. } => "thinking".into(),
                Body::Command { .. } => "command".into(),
                Body::Notice { .. } => "notice".into(),
                Body::Tool { tool, status, .. } => format!("tool:{tool}:{status:?}"),
                Body::Question { state, .. } => format!("question:{state:?}"),
            };
            *counts.entry(kind).or_default() += 1;
        }
        eprintln!("items: {} (seq {}), model {:?}, effort {:?}", s.items.len(), s.seq, s.model, s.effort);
        for (k, n) in &counts {
            eprintln!("  {n:5}  {k}");
        }
        for i in s.items.iter().filter(|i| matches!(i.body, Body::User { .. })).take(3) {
            if let Body::User { text, images } = &i.body {
                eprintln!("user> {} [{images} img]", clip(first_line(text), 90));
            }
        }
        if let Some(Body::Assistant { text }) = s.items.iter().rev().map(|i| &i.body).find(|b| matches!(b, Body::Assistant { .. })) {
            eprintln!("last assistant> {}", clip(first_line(text), 90));
        }
    }

    #[test]
    fn codex_shell_wrappers_are_dropped_from_commands() {
        assert_eq!(bare_command("/bin/zsh -lc eza -1 src"), "eza -1 src");
        assert_eq!(bare_command("bash -lc 'cargo test --quiet'"), "cargo test --quiet");
        assert_eq!(bare_command("/usr/bin/sh -c \"echo hi\""), "echo hi");
        assert_eq!(bare_command("eza -1 src"), "eza -1 src");
        assert_eq!(bare_command("zsh"), "zsh");
        assert_eq!(bare_command("zsh -lc"), "zsh -lc");
        assert_eq!(bare_command("python3 -c print(1)"), "python3 -c print(1)");
    }

    #[test]
    fn text_helpers() {
        assert_eq!(strip_tagged("a<x>b</x>c<x>d</x>e", "x"), "ace");
        assert_eq!(strip_tagged("a<x>unterminated", "x"), "a");
        assert_eq!(between("<a>hi</a>", "<a>", "</a>"), Some("hi"));
        assert_eq!(between("<a>hi", "<a>", "</a>"), None);
        assert_eq!(short_path("/a/b/c.rs"), "b/c.rs");
        assert_eq!(short_path("c.rs"), "c.rs");
        assert_eq!(clip("héllo wörld", 5), "héllo…");
        assert_eq!(clip("short", 50), "short");
    }
}
