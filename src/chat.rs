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
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::{
    AppState, agentlog,
    config::Agent,
    modes, screen, sessions,
    subagents::{self, SubAgent},
    chatimg, tags, usage,
};

/// Items kept per session; older ones are dropped (the client keeps what it already has).
const MAX_ITEMS: usize = 2000;
/// Longest tool input or output sent to the browser.
const PREVIEW_CHARS: usize = 2000;
/// Longest side of an edit shown as a diff.
const DIFF_CHARS: usize = 8000;
/// Longest single message.
const TEXT_CHARS: usize = 20_000;
/// Longest one-line preview of a conversation.
const PREVIEW_LINE: usize = 140;
/// Longest result shown on a helper agent's card.
const SUMMARY_TEXT: usize = 600;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// One entry of the agent's to-do list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Todo {
    pub content: String,
    pub status: TodoStatus,
    /// What it is doing right now ("Running the tests"), while in progress.
    pub active: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// `images` counts every picture attached; `pics` are the ones that can be shown (see
    /// `chatimg`). `pastes` are blocks of text pasted into the message.
    User {
        text: String,
        images: u32,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        pics: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        pastes: Vec<crate::tags::Paste>,
    },
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
        /// Pictures the tool returned (a screenshot, a viewed image), as URLs.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        pics: Vec<String>,
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

/// A helper agent Codex started, as the parent conversation's log records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub thread_id: String,
    /// Its name, the last part of its path (`/root/search_facts`).
    pub name: String,
    pub depth: u32,
    /// The id of the log item that announced it.
    pub started_id: String,
    pub done: bool,
}

/// What a conversation has done so far, in a few fields.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub tools: u32,
    pub last_action: Option<String>,
    /// The last thing the agent said.
    pub result: Option<String>,
    pub first_ts: Option<i64>,
}

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
    /// The agent's latest to-do list (Claude's `TodoWrite`).
    todos: Vec<Todo>,
    /// The latest plan the agent put forward (Claude's `ExitPlanMode`, Codex's `Plan`), as Markdown.
    plan: Option<String>,
    /// This is a helper agent's own log: Claude marks every line of those as a side chain.
    sidechain: bool,
    /// Helpers Codex started (Codex only).
    spawns: Vec<Spawn>,
    /// The `!` command whose output has not arrived yet: its bubble is where the output goes.
    pending_bash: Option<String>,
    /// Where pictures in the log are kept so the browser can show them. Without it they are counted.
    images: Option<Arc<chatimg::Store>>,
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
            todos: Vec::new(),
            plan: None,
            sidechain: false,
            spawns: Vec::new(),
            pending_bash: None,
            images: None,
        }
    }

    pub fn with_images(mut self, images: Option<Arc<chatimg::Store>>) -> Self {
        self.images = images;
        self
    }

    /// The state for a helper agent's own log file.
    pub fn new_sub(agent: Agent, path: PathBuf) -> Self {
        let mut s = Self::new(agent, "");
        s.path = Some(path);
        s.sidechain = true;
        s
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    #[cfg(test)]
    pub fn spawns(&self) -> &[Spawn] {
        &self.spawns
    }

    pub fn has_item(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }

    pub fn summary(&self) -> Summary {
        let tool = |i: &Item| match &i.body {
            Body::Tool { summary, .. } => Some(summary.clone()),
            _ => None,
        };
        Summary {
            tools: self.items.iter().filter(|i| matches!(i.body, Body::Tool { .. })).count() as u32,
            last_action: self.items.iter().rev().find_map(tool),
            result: self.items.iter().rev().find_map(|i| match &i.body {
                Body::Assistant { text } => Some(clip(text.trim(), SUMMARY_TEXT)).filter(|t| !t.is_empty()),
                _ => None,
            }),
            first_ts: self.items.iter().find_map(|i| i.ts),
        }
    }

    /// The last thing said, in one line, for the session list.
    fn preview(&self) -> Option<String> {
        self.items.iter().rev().find_map(|i| match &i.body {
            Body::Assistant { text } | Body::User { text, .. } => {
                let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
                Some(clip(&one, PREVIEW_LINE)).filter(|s| !s.is_empty())
            }
            _ => None,
        })
    }

    fn reset(&mut self) {
        let (agent, id, old_epoch, sidechain) = (self.agent, std::mem::take(&mut self.conv_id), self.epoch, self.sidechain);
        *self = Self::new(agent, &id);
        self.sidechain = sidechain;
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
        if v["isSidechain"].as_bool() == Some(true) && !self.sidechain {
            return; // a sub-agent's own conversation, which has its own log
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
        match name {
            "TodoWrite" => self.todos = parse_todos(input),
            "ExitPlanMode" => {
                let plan = str_of(input, "plan").trim();
                if !plan.is_empty() {
                    self.plan = Some(clip(plan, TEXT_CHARS));
                }
            }
            _ => {}
        }
        let (summary, shown_input, diff) = claude_tool_summary(name, input);
        self.add(
            id,
            ts,
            Body::Tool { tool: name.to_owned(), summary, input: shown_input, output: None, status: ToolStatus::Running, diff, pics: Vec::new() },
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
                self.user_text(uuid.to_owned(), ts, t, 0, Vec::new());
            }
            return;
        };
        let mut texts: Vec<&str> = Vec::new();
        let mut images = 0;
        let mut pics = Vec::new();
        for b in blocks {
            match b["type"].as_str() {
                Some("text") => texts.push(str_of(b, "text")),
                Some("image") => {
                    images += 1;
                    pics.extend(self.claude_pic(b));
                }
                Some("tool_result") => self.claude_tool_result(b, v),
                _ => {}
            }
        }
        if !texts.is_empty() || images > 0 {
            self.user_text(uuid.to_owned(), ts, &texts.join("\n"), images, pics);
        }
    }

    /// The URL of a Claude `image` block, once its base64 is saved.
    fn claude_pic(&self, block: &Value) -> Option<String> {
        let source = &block["source"];
        match source["type"].as_str() {
            Some("base64") => self.images.as_ref()?.save_base64(source["data"].as_str()?),
            _ => None,
        }
    }

    fn claude_tool_result(&mut self, b: &Value, line: &Value) {
        let id = str_of(b, "tool_use_id");
        let is_error = b["is_error"].as_bool() == Some(true);
        let output = tags::clean_output(&text_of(&b["content"]));
        let pics: Vec<String> = b["content"].as_array().into_iter().flatten().filter(|c| c["type"] == "image").filter_map(|c| self.claude_pic(c)).collect();
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
            Body::Tool { output: o, status, pics: p, .. } => {
                *o = Some(clip(output.trim(), PREVIEW_CHARS)).filter(|s| !s.is_empty());
                *status = if is_error { ToolStatus::Error } else { ToolStatus::Ok };
                *p = pics;
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

    /// A message you typed, or one of the wrappers the agents put around slash commands, `!`
    /// commands, finished background tasks and pasted text (see `tags`).
    fn user_text(&mut self, id: String, ts: Option<i64>, text: &str, images: u32, pics: Vec<String>) {
        let p = tags::parse(text);
        if let Some((name, args)) = &p.command {
            self.add(id, ts, Body::Command { text: format!("{name} {args}").trim().to_owned() });
            return;
        }
        if p.stdout.is_some() || p.stderr.is_some() {
            if let Some(out) = &p.stdout {
                self.note_switch(out);
            }
            let shown = [p.stdout.as_deref(), p.stderr.as_deref()].into_iter().flatten().map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
            if !shown.is_empty() {
                self.add(id, ts, Body::Notice { text: clip(&shown, PREVIEW_CHARS) });
            }
            return;
        }
        if let Some(cmd) = &p.bash_input {
            self.bash_started(id, ts, cmd);
            return;
        }
        if p.bash_stdout.is_some() || p.bash_stderr.is_some() {
            self.bash_finished(id, ts, p.bash_stdout.as_deref().unwrap_or(""), p.bash_stderr.as_deref().unwrap_or(""));
            return;
        }
        if let Some(task) = &p.task {
            self.add(id, ts, Body::Notice { text: clip(&task.line(), PREVIEW_CHARS) });
            return;
        }
        let said_nothing = p.text.is_empty() && p.pastes.is_empty() && images == 0;
        if p.aborted && said_nothing {
            self.add(id, ts, Body::Notice { text: "Interrupted".into() });
            return;
        }
        let t = p.text.as_str();
        // Claude records an interruption as a line from "you"; it is a note, not something you typed.
        if t.starts_with("[Request interrupted by user") {
            self.add(id, ts, Body::Notice { text: "Interrupted".into() });
            return;
        }
        // Claude describes an attached image in a text block of its own.
        let t = t.lines().filter(|l| !(l.starts_with("[Image") && l.ends_with(']'))).collect::<Vec<_>>().join("\n");
        let t = t.trim();
        if t.is_empty() && images == 0 && p.pastes.is_empty() {
            return;
        }
        let pastes = p.pastes.into_iter().map(|mut x| {
            x.text = clip(&x.text, TEXT_CHARS);
            x
        });
        self.add(id, ts, Body::User { text: clip(t, TEXT_CHARS), images, pics, pastes: pastes.collect() });
    }

    /// A `!` command you ran in Claude: shown like a Bash tool call, which its output fills in.
    fn bash_started(&mut self, id: String, ts: Option<i64>, command: &str) {
        self.pending_bash = Some(id.clone());
        self.add(
            id,
            ts,
            Body::Tool {
                tool: "Bash".into(),
                summary: clip(first_line(command), 120),
                input: Some(clip(command, PREVIEW_CHARS)),
                output: None,
                status: ToolStatus::Ok,
                diff: None,
                pics: Vec::new(),
            },
        );
    }

    fn bash_finished(&mut self, id: String, ts: Option<i64>, stdout: &str, stderr: &str) {
        let (out, err) = (stdout.trim(), stderr.trim());
        let shown = [out, err].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
        if let Some(pending) = self.pending_bash.take() {
            let failed = out.is_empty() && !err.is_empty();
            let filled = self.modify(&pending, |body| {
                if let Body::Tool { output, status, .. } = body {
                    *output = Some(clip(&shown, PREVIEW_CHARS)).filter(|s| !s.is_empty());
                    *status = if failed { ToolStatus::Error } else { ToolStatus::Ok };
                }
            });
            if filled {
                return;
            }
        }
        // Its command is not in the log (it started mid-way): the output on its own.
        if !shown.is_empty() {
            self.add(id, ts, Body::Notice { text: clip(&shown, PREVIEW_CHARS) });
        }
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

    /// The URL of a Codex image block: a file it was given, or a data URL.
    fn codex_pic(&self, block: &Value) -> Option<String> {
        let store = self.images.as_ref()?;
        if let Some(path) = block["path"].as_str() {
            return store.save_file(path);
        }
        let url = block["image_url"].as_str().or_else(|| block["image_url"]["url"].as_str())?;
        store.save_data_url(url)
    }

    fn codex_item(&mut self, item: &Value, ts: Option<i64>) {
        let id = str_of(item, "id").to_owned();
        if id.is_empty() {
            return;
        }
        match item["type"].as_str() {
            Some("UserMessage") => {
                let t = text_of(&item["content"]);
                let shots: Vec<&Value> = item["content"].as_array().into_iter().flatten().filter(|b| matches!(b["type"].as_str(), Some("local_image" | "image" | "input_image"))).collect();
                let pics: Vec<String> = shots.iter().filter_map(|b| self.codex_pic(b)).collect();
                if !t.trim().is_empty() || !shots.is_empty() {
                    self.user_text(id, ts, &t, shots.len() as u32, pics);
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
                        pics: Vec::new(),
                    },
                );
            }
            Some("FileChange") => self.codex_file_change(item, &id, ts),
            // Only the shapes understood here; anything else of that type is shown like any other item.
            Some("SubAgentActivity") if self.codex_sub_agent(item, &id, ts) => {}
            Some("Plan") => {
                let t = str_of(item, "text").trim();
                if !t.is_empty() {
                    self.plan = Some(clip(t, TEXT_CHARS));
                    self.add(
                        id,
                        ts,
                        Body::Tool {
                            tool: "Plan".into(),
                            summary: "Proposed a plan".into(),
                            input: Some(clip(t, PREVIEW_CHARS)),
                            output: None,
                            status: ToolStatus::Ok,
                            diff: None,
                            pics: Vec::new(),
                        },
                    );
                }
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
                        pics: Vec::new(),
                    },
                );
            }
            None => {}
        }
    }
}

/// The to-do list in a `TodoWrite` call. Entries without text are dropped.
fn parse_todos(input: &Value) -> Vec<Todo> {
    input["todos"]
        .as_array()
        .map(|ts| {
            ts.iter()
                .filter_map(|t| {
                    let content = str_of(t, "content").trim();
                    if content.is_empty() {
                        return None;
                    }
                    let status = match str_of(t, "status") {
                        "in_progress" => TodoStatus::InProgress,
                        "completed" => TodoStatus::Completed,
                        _ => TodoStatus::Pending,
                    };
                    let active = Some(str_of(t, "activeForm").trim()).filter(|a| !a.is_empty()).map(|a| clip(a, 200));
                    Some(Todo { content: clip(content, 300), status, active })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl ChatState {
    /// Codex tells the parent conversation when a helper starts and finishes. A start becomes a
    /// card like Claude's Agent tool; messages sent to a helper are noise and are not shown.
    /// Returns whether the item was one it understands.
    fn codex_sub_agent(&mut self, item: &Value, id: &str, ts: Option<i64>) -> bool {
        let thread = str_of(item, "agent_thread_id");
        let path = str_of(item, "agent_path");
        let name = path.rsplit('/').next().unwrap_or("");
        let kind = str_of(item, "kind");
        if thread.is_empty() || name.is_empty() || !matches!(kind, "started" | "completed" | "interacted") {
            return false;
        }
        if name == "root" {
            return true; // activity of the main thread itself
        }
        match kind {
            "started" => {
                self.spawns.retain(|s| s.thread_id != thread);
                self.spawns.push(Spawn {
                    thread_id: thread.to_owned(),
                    name: name.to_owned(),
                    depth: path.matches('/').count().saturating_sub(1) as u32,
                    started_id: id.to_owned(),
                    done: false,
                });
                self.add(
                    id.to_owned(),
                    ts,
                    Body::Tool {
                        tool: "Agent".into(),
                        summary: name.replace('_', " "),
                        input: None,
                        output: None,
                        status: ToolStatus::Running,
                        diff: None,
                        pics: Vec::new(),
                    },
                );
            }
            "completed" => {
                if let Some(sp) = self.spawns.iter_mut().find(|s| s.thread_id == thread) {
                    sp.done = true;
                    let started = sp.started_id.clone();
                    self.modify(&started, |b| {
                        if let Body::Tool { status, .. } = b {
                            *status = ToolStatus::Ok;
                        }
                    });
                }
            }
            _ => {} // a message sent to a helper: noise
        }
        true
    }

    /// Codex reports a patch as one item holding every file it touched. Show each file as the same
    /// kind of edit Claude's tools produce, so both agents' changes read (and add up) alike.
    fn codex_file_change(&mut self, item: &Value, id: &str, ts: Option<i64>) {
        let Some(changes) = item["changes"].as_object() else { return };
        let failed = matches!(item["status"].as_str(), Some("Failed" | "failed" | "Declined" | "declined"));
        let output = Some(clip(str_of(item, "stderr").trim(), PREVIEW_CHARS)).filter(|s| failed && !s.is_empty());
        for (n, (path, c)) in changes.iter().enumerate() {
            let content = clip(str_of(c, "content"), DIFF_CHARS);
            let (tool, verb, edits) = match c["type"].as_str() {
                Some("add") => ("Write", "Created", vec![EditPair { old: String::new(), new: content }]),
                Some("delete") => ("Delete", "Deleted", vec![EditPair { old: content, new: String::new() }]),
                _ => ("Edit", "Edited", unified_pairs(str_of(c, "unified_diff"))),
            };
            self.add(
                format!("{id}:{n}"),
                ts,
                Body::Tool {
                    tool: tool.into(),
                    summary: format!("{verb} {}", short_path(path)),
                    input: None,
                    output: output.clone(),
                    status: if failed { ToolStatus::Error } else { ToolStatus::Ok },
                    diff: Some(Diff { path: path.clone(), edits }),
                    pics: Vec::new(),
                },
            );
        }
    }
}

/// A unified diff as one before/after pair per hunk (context lines are in both sides).
fn unified_pairs(diff: &str) -> Vec<EditPair> {
    let mut out = Vec::new();
    let (mut old, mut new): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    let mut flush = |old: &mut Vec<&str>, new: &mut Vec<&str>| {
        if !old.is_empty() || !new.is_empty() {
            out.push(EditPair { old: clip(&old.join("\n"), DIFF_CHARS), new: clip(&new.join("\n"), DIFF_CHARS) });
        }
        old.clear();
        new.clear();
    };
    let mut in_hunk = false;
    for line in diff.lines() {
        if line.starts_with("@@") {
            flush(&mut old, &mut new);
            in_hunk = true;
        } else if !in_hunk || line.starts_with('\\') {
            continue; // file headers before the first hunk, and "\ No newline at end of file"
        } else if let Some(l) = line.strip_prefix('+') {
            new.push(l);
        } else if let Some(l) = line.strip_prefix('-') {
            old.push(l);
        } else {
            let l = line.strip_prefix(' ').unwrap_or(line);
            old.push(l);
            new.push(l);
        }
    }
    flush(&mut old, &mut new);
    out
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
    images: Option<Arc<chatimg::Store>>,
}

impl ChatCache {
    pub fn with_images(images: Arc<chatimg::Store>) -> Self {
        Self { images: Some(images), ..Self::default() }
    }

    /// The saved picture a URL from this cache points at.
    pub fn image_path(&self, name: &str) -> Option<PathBuf> {
        self.images.as_ref()?.locate(name)
    }
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
    /// The agent's current to-do list, if it keeps one.
    pub todos: Vec<Todo>,
    /// The latest plan the agent put forward, as Markdown.
    pub plan: Option<String>,
    /// Helper agents this conversation started, oldest first.
    pub agents: Vec<SubAgent>,
}

impl ChatResponse {
    /// What `state` holds beyond `after` (everything, if the client's `epoch` is not this parse's).
    /// The screen-derived fields are left empty for the caller to fill.
    fn of(state: &ChatState, agent: Agent, ready: bool, epoch: Option<u64>, after: u64) -> Self {
        let reset = epoch != Some(state.epoch);
        Self {
            epoch: state.epoch,
            seq: state.seq,
            reset,
            items: if reset { state.since(0) } else { state.since(after) },
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
            todos: state.todos.clone(),
            plan: state.plan.clone(),
            agents: Vec::new(),
        }
    }
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

    /// The parsed log for `session`, started afresh when it now belongs to another conversation.
    fn state_for<'a>(
        states: &'a mut HashMap<String, ChatState>,
        images: &Option<Arc<chatimg::Store>>,
        session: &str,
        agent: Agent,
        conv_id: &str,
    ) -> &'a mut ChatState {
        let stale = states.get(session).is_none_or(|s| s.agent != agent || s.conv_id != conv_id);
        if stale {
            states.insert(session.to_owned(), ChatState::new(agent, conv_id).with_images(images.clone()));
        }
        let Some(state) = states.get_mut(session) else { unreachable!("inserted above") };
        state
    }

    /// One line of the latest message and how many helper agents are running, after reading any
    /// new log lines. Blocking: reads the logs. Call from `spawn_blocking`.
    fn overview(&self, session: &str, agent: Agent, conv_id: &str) -> (Option<String>, u32) {
        let mut states = lock(&self.states);
        let state = Self::state_for(&mut states, &self.images, session, agent, conv_id);
        if state.update().is_err() {
            return (None, 0);
        }
        let preview = state.preview();
        let (path, spawns) = (state.path.clone(), state.spawns.clone());
        let agents = subagents::collect(&mut states, session, agent, path.as_deref(), &spawns, codex_sessions().as_deref(), subagents::now_secs());
        (preview, agents.iter().filter(|a| a.running).count() as u32)
    }

    /// Every helper agent of the conversation. Blocking.
    fn agents(&self, session: &str, agent: Agent, conv_id: &str) -> Vec<SubAgent> {
        let mut states = lock(&self.states);
        let state = Self::state_for(&mut states, &self.images, session, agent, conv_id);
        let _ = state.update();
        let (path, spawns) = (state.path.clone(), state.spawns.clone());
        subagents::collect(&mut states, session, agent, path.as_deref(), &spawns, codex_sessions().as_deref(), subagents::now_secs())
    }

    /// One helper's transcript, in the same shape as a conversation. None if there is no such helper.
    /// Blocking.
    fn agent_chat(&self, session: &str, agent: Agent, conv_id: &str, aid: &str, epoch: Option<u64>, after: u64) -> Option<ChatResponse> {
        let mut states = lock(&self.states);
        let state = Self::state_for(&mut states, &self.images, session, agent, conv_id);
        let _ = state.update();
        let (path, spawns) = (state.path.clone(), state.spawns.clone());
        // This also makes sure the helper's own log has been read up to now.
        let list = subagents::collect(&mut states, session, agent, path.as_deref(), &spawns, codex_sessions().as_deref(), subagents::now_secs());
        let info = list.iter().find(|a| a.id == aid)?;
        let sub = states.get(&subagents::key(session, aid))?;
        let mut response = ChatResponse::of(sub, agent, info.has_log, epoch, after);
        response.agents = list.into_iter().filter(|a| a.parent.as_deref() == Some(aid)).collect();
        Some(response)
    }

    /// Blocking: reads the log. Call from `spawn_blocking`.
    fn read(&self, session: &str, agent: Agent, conv_id: &str, epoch: Option<u64>, after: u64) -> (ChatResponse, bool) {
        let mut states = lock(&self.states);
        let state = Self::state_for(&mut states, &self.images, session, agent, conv_id);
        let ready = state.update().unwrap_or_else(|e| {
            tracing::warn!("reading the {} log for {session}: {e}", conv_id);
            false
        });
        let mut response = ChatResponse::of(state, agent, ready, epoch, after);
        let (path, spawns) = (state.path.clone(), state.spawns.clone());
        response.agents = subagents::collect(&mut states, session, agent, path.as_deref(), &spawns, codex_sessions().as_deref(), subagents::now_secs());
        (response, ready)
    }
}

/// The agent and conversation id of a Claude or Codex session whose conversation is known.
fn agent_of(st: &AppState, session: &str) -> Option<(Agent, String)> {
    st.sessions
        .get(session)
        .and_then(|m| Some((m.agent()?, m.agent_session_id.clone()?)))
        .filter(|(a, id)| *a != Agent::Other && sessions::valid_agent_id(id))
}

/// A one-line preview of the latest message in `session`, and how many of its helper agents are
/// running, for the session list.
pub async fn overview(st: &Arc<AppState>, session: &str) -> (Option<String>, u32) {
    let Some((agent, conv_id)) = agent_of(st, session) else { return (None, 0) };
    let cache = st.chat.clone();
    let name = session.to_owned();
    tokio::task::spawn_blocking(move || cache.overview(&name, agent, &conv_id)).await.unwrap_or((None, 0))
}

/// What you have typed to `session`'s agent, oldest first. Empty for a plain shell.
pub async fn user_messages(st: &Arc<AppState>, session: &str) -> Vec<crate::turns::UserMsg> {
    let Some((agent, conv_id)) = agent_of(st, session) else { return Vec::new() };
    let cache = st.chat.clone();
    let name = session.to_owned();
    tokio::task::spawn_blocking(move || {
        cache
            .read(&name, agent, &conv_id, None, 0)
            .0
            .items
            .into_iter()
            .filter_map(|i| match i.body {
                Body::User { text, .. } => Some(crate::turns::UserMsg { id: i.id, ts: i.ts, text }),
                _ => None,
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// The helper agents `session` has started.
pub async fn agents(st: &Arc<AppState>, session: &str) -> Vec<SubAgent> {
    let Some((agent, conv_id)) = agent_of(st, session) else { return Vec::new() };
    let cache = st.chat.clone();
    let name = session.to_owned();
    tokio::task::spawn_blocking(move || cache.agents(&name, agent, &conv_id)).await.unwrap_or_default()
}

/// What changed in one helper's transcript since `(epoch, after)`.
pub async fn agent_chat(st: &Arc<AppState>, session: &str, aid: &str, epoch: Option<u64>, after: u64) -> Option<ChatResponse> {
    let (agent, conv_id) = agent_of(st, session)?;
    let cache = st.chat.clone();
    let (name, aid) = (session.to_owned(), aid.to_owned());
    tokio::task::spawn_blocking(move || cache.agent_chat(&name, agent, &conv_id, &aid, epoch, after)).await.ok()?
}

fn codex_sessions() -> Option<PathBuf> {
    agentlog::codex_dir().map(|d| d.join("sessions"))
}

/// What changed in `session`'s conversation since `(epoch, after)`.
pub async fn snapshot(st: &Arc<AppState>, session: &str, epoch: Option<u64>, after: u64) -> Result<ChatResponse> {
    let (agent, conv_id) = agent_of(st, session).map_or((None, None), |(a, id)| (Some(a), Some(id)));
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
            todos: Vec::new(),
            plan: None,
            agents: Vec::new(),
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
                &Body::User { text: "hello there".into(), images: 0, pics: vec![], pastes: vec![] },
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
                &Body::User { text: "real question".into(), images: 0, pics: vec![], pastes: vec![] },
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
                &Body::User { text: "a real message that mentions [Request interrupted by user]".into(), images: 0, pics: vec![], pastes: vec![] },
            ]
        );
    }

    fn tool(summary: &str, input: &str, output: Option<&str>, status: ToolStatus) -> Body {
        Body::Tool { tool: "Bash".into(), summary: summary.into(), input: Some(input.into()), output: output.map(Into::into), status, diff: None, pics: vec![] }
    }

    #[test]
    fn text_helpers() {
        assert_eq!(short_path("/a/b/c.rs"), "b/c.rs");
        assert_eq!(short_path("c.rs"), "c.rs");
        assert_eq!(clip("héllo wörld", 5), "héllo…");
        assert_eq!(clip("short", 50), "short");
    }

    #[test]
    fn a_bang_command_and_its_output_become_one_bash_card() {
        let mut s = claude();
        feed(
            &mut s,
            &[
                user_text("c1", "<local-command-caveat>The command below was run directly in Claude Code</local-command-caveat>"),
                user_text("i1", "<bash-input> ls -1 | head -2 </bash-input>"),
                user_text("o1", "<bash-stdout>a.txt\nb.txt</bash-stdout><bash-stderr></bash-stderr>"),
                user_text("i2", "<bash-input>cat missing</bash-input>"),
                user_text("o2", "<bash-stdout></bash-stdout><bash-stderr>cat: missing: No such file</bash-stderr>"),
                user_text("i3", "<bash-input>true</bash-input>"),
                user_text("o3", "<bash-stdout></bash-stdout><bash-stderr></bash-stderr>"),
            ],
        );
        assert_eq!(
            bodies(&s),
            vec![
                &tool("ls -1 | head -2", "ls -1 | head -2", Some("a.txt\nb.txt"), ToolStatus::Ok),
                &tool("cat missing", "cat missing", Some("cat: missing: No such file"), ToolStatus::Error),
                &tool("true", "true", None, ToolStatus::Ok),
            ]
        );
    }

    #[test]
    fn bash_output_without_its_command_is_a_note() {
        let mut s = claude();
        feed(&mut s, &[user_text("o1", "<bash-stdout>left over</bash-stdout><bash-stderr></bash-stderr>"), user_text("o2", "<bash-stdout></bash-stdout>")]);
        assert_eq!(bodies(&s), vec![&Body::Notice { text: "left over".into() }]);
    }

    #[test]
    fn pasted_text_is_kept_apart_from_what_you_typed() {
        let mut s = claude();
        feed(
            &mut s,
            &[
                user_text("p1", "\n\n<pasted_content id=\"6e14\">\n~/code main*\n❯ ls\n</pasted_content id=\"6e14\">\n"),
                user_text("p2", "why?\n<pasted_content id=\"a1\">boom</pasted_content>"),
            ],
        );
        let paste = |id: &str, text: &str| crate::tags::Paste { id: Some(id.into()), text: text.into() };
        assert_eq!(
            bodies(&s),
            vec![
                &Body::User { text: String::new(), images: 0, pics: vec![], pastes: vec![paste("6e14", "~/code main*\n❯ ls")] },
                &Body::User { text: "why?".into(), images: 0, pics: vec![], pastes: vec![paste("a1", "boom")] },
            ]
        );
    }

    #[test]
    fn a_finished_background_task_is_a_notice() {
        let mut s = claude();
        feed(
            &mut s,
            &[user_text(
                "t1",
                "<task-notification>\n<task-id>b4y</task-id>\n<tool-use-id>toolu_01</tool-use-id>\n<output-file>/tmp/x</output-file>\n<status>completed</status>\n<summary>Background command \"Find it\" completed (exit code 0)</summary>\n</task-notification>",
            )],
        );
        assert_eq!(bodies(&s), vec![&Body::Notice { text: "Background command \"Find it\" completed (exit code 0)".into() }]);
    }

    #[test]
    fn a_slash_commands_error_output_is_shown_too() {
        let mut s = claude();
        feed(&mut s, &[user_text("e1", "<local-command-stdout></local-command-stdout><local-command-stderr>Unknown command</local-command-stderr>")]);
        assert_eq!(bodies(&s), vec![&Body::Notice { text: "Unknown command".into() }]);
    }

    #[test]
    fn tool_results_lose_claudes_wrappers_but_keep_file_contents() {
        let mut s = claude();
        feed(
            &mut s,
            &[
                assistant("a1", json!({ "type": "tool_use", "id": "t1", "name": "Read", "input": { "file_path": "/x/a.rs" } })),
                json!({ "type": "user", "uuid": "r1", "message": { "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "let a = \"<system-reminder>x</system-reminder>\";\n\n<system-reminder>be careful</system-reminder>" } ] } }),
                assistant("a2", json!({ "type": "tool_use", "id": "t2", "name": "Read", "input": { "file_path": "/x/b.rs" } })),
                json!({ "type": "user", "uuid": "r2", "message": { "content": [
                    { "type": "tool_result", "tool_use_id": "t2", "is_error": true, "content": "<tool_use_error>File does not exist.</tool_use_error>" } ] } }),
            ],
        );
        let outs: Vec<_> = s.items.iter().filter_map(|i| if let Body::Tool { output, .. } = &i.body { output.clone() } else { None }).collect();
        assert_eq!(outs, vec!["let a = \"<system-reminder>x</system-reminder>\";".to_owned(), "File does not exist.".to_owned()]);
    }

    #[test]
    fn codex_interruptions_and_review_prompts_are_tidied() {
        let mut s = ChatState::new(Agent::Codex, "thread");
        let done = |item: Value| json!({ "type": "event_msg", "payload": { "type": "item_completed", "item": item } });
        let user = |id: &str, text: &str| done(json!({ "type": "UserMessage", "id": id, "content": [{ "type": "text", "text": text }] }));
        feed(
            &mut s,
            &[
                user("m1", "<turn_aborted>\nThe user interrupted the previous turn.\n</turn_aborted>"),
                user("m2", "<task>\nRun a stop-gate review.\n</task>"),
                user("m3", "<environment_context><cwd>/x</cwd></environment_context>"),
                user("m4", "plain question about <b>html</b>"),
            ],
        );
        assert_eq!(
            bodies(&s),
            vec![
                &Body::Notice { text: "Interrupted".into() },
                &Body::User { text: "Run a stop-gate review.".into(), images: 0, pics: vec![], pastes: vec![] },
                &Body::User { text: "plain question about <b>html</b>".into(), images: 0, pics: vec![], pastes: vec![] },
            ]
        );
    }

    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89,
    ];

    fn with_store(mut s: ChatState, name: &str) -> (ChatState, PathBuf) {
        let d = std::env::temp_dir().join(format!("ttm-chat-img-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        s = s.with_images(Some(Arc::new(chatimg::Store::new(&d))));
        (s, d)
    }

    fn b64(bytes: &[u8]) -> String {
        use base64::{Engine, engine::general_purpose::STANDARD};
        STANDARD.encode(bytes)
    }

    #[test]
    fn a_picture_you_attach_is_saved_and_shown() {
        let (mut s, d) = with_store(claude(), "user");
        feed(
            &mut s,
            &[json!({ "type": "user", "uuid": "u1", "message": { "content": [
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64(PNG) } },
                { "type": "image", "source": { "type": "url", "url": "https://example.com/x.png" } },
                { "type": "text", "text": "what is this?" } ] } })],
        );
        let Body::User { images, pics, text, .. } = &s.items[0].body else { panic!("not a user message") };
        assert_eq!((text.as_str(), *images), ("what is this?", 2), "both pictures are counted");
        assert_eq!(pics.len(), 1, "the one that can be shown is shown");
        assert!(pics[0].starts_with(chatimg::URL_PREFIX));
        assert_eq!(std::fs::read_dir(d.join("chat-images")).unwrap().count(), 1);
    }

    #[test]
    fn a_screenshot_a_tool_returns_is_shown_on_its_card() {
        let (mut s, _d) = with_store(claude(), "tool");
        feed(
            &mut s,
            &[
                assistant("a1", json!({ "type": "tool_use", "id": "t1", "name": "mcp__browser__screenshot", "input": {} })),
                json!({ "type": "user", "uuid": "r1", "message": { "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": [
                        { "type": "text", "text": "Took a screenshot" },
                        { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64(PNG) } } ] } ] } }),
            ],
        );
        let Body::Tool { output, pics, .. } = &s.items[0].body else { panic!("not a tool") };
        assert_eq!(output.as_deref(), Some("Took a screenshot"));
        assert_eq!(pics.len(), 1);
    }

    #[test]
    fn without_a_store_pictures_are_only_counted() {
        let mut s = claude();
        feed(
            &mut s,
            &[json!({ "type": "user", "uuid": "u1", "message": { "content": [
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64(PNG) } } ] } })],
        );
        assert_eq!(bodies(&s), vec![&Body::User { text: String::new(), images: 1, pics: vec![], pastes: vec![] }]);
    }

    #[test]
    fn codex_pictures_come_from_a_file_or_a_data_url() {
        let (mut s, d) = with_store(ChatState::new(Agent::Codex, "thread"), "codex");
        let shot = d.join("Screenshot 1.png");
        std::fs::write(&shot, PNG).unwrap();
        let done = |item: Value| json!({ "type": "event_msg", "payload": { "type": "item_completed", "item": item } });
        feed(
            &mut s,
            &[
                done(json!({ "type": "UserMessage", "id": "m1", "content": [
                    { "type": "local_image", "path": shot.to_str().unwrap() },
                    { "type": "local_image", "path": "/gone/away.png" },
                    { "type": "text", "text": "look" } ] })),
                done(json!({ "type": "UserMessage", "id": "m2", "content": [
                    { "type": "input_image", "image_url": format!("data:image/png;base64,{}", b64(PNG)) } ] })),
            ],
        );
        let shown: Vec<(u32, usize)> = s.items.iter().filter_map(|i| if let Body::User { images, pics, .. } = &i.body { Some((*images, pics.len())) } else { None }).collect();
        assert_eq!(shown, vec![(2, 1), (1, 1)]);
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
        assert_eq!(bodies(&s), vec![&Body::User { text: "what is this?".into(), images: 1, pics: vec![], pastes: vec![] }]);
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
            if let Body::User { text, images, .. } = &i.body {
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

    // ---------------------------------------------------- todos, plan, preview

    fn tool_use(uuid: &str, name: &str, input: Value) -> Value {
        assistant(uuid, json!({ "type": "tool_use", "id": format!("t-{uuid}"), "name": name, "input": input }))
    }

    #[test]
    fn todo_write_becomes_a_structured_list_and_the_latest_call_wins() {
        let mut s = claude();
        feed(
            &mut s,
            &[
                tool_use("a1", "TodoWrite", json!({ "todos": [
                    { "content": "Write tests", "status": "in_progress", "activeForm": "Writing tests" },
                    { "content": "Ship it", "status": "pending", "activeForm": "Shipping" },
                    { "content": "  ", "status": "pending" },
                ] })),
            ],
        );
        assert_eq!(
            s.todos,
            vec![
                Todo { content: "Write tests".into(), status: TodoStatus::InProgress, active: Some("Writing tests".into()) },
                Todo { content: "Ship it".into(), status: TodoStatus::Pending, active: Some("Shipping".into()) },
            ],
            "an entry without text is dropped"
        );
        feed(&mut s, &[tool_use("a2", "TodoWrite", json!({ "todos": [{ "content": "Ship it", "status": "completed" }] }))]);
        assert_eq!(s.todos, vec![Todo { content: "Ship it".into(), status: TodoStatus::Completed, active: None }]);
        // The call still shows in the conversation as its one-line summary.
        assert!(s.items.iter().any(|i| matches!(&i.body, Body::Tool { tool, .. } if tool == "TodoWrite")));
    }

    #[test]
    fn an_unknown_todo_status_counts_as_pending() {
        let todos = parse_todos(&json!({ "todos": [{ "content": "x", "status": "someday" }] }));
        assert_eq!(todos[0].status, TodoStatus::Pending);
        assert!(parse_todos(&json!({})).is_empty());
    }

    #[test]
    fn exit_plan_mode_keeps_the_plan_and_a_later_plan_replaces_it() {
        let mut s = claude();
        feed(&mut s, &[tool_use("a1", "ExitPlanMode", json!({ "plan": "# Plan\n\n1. Do it\n" }))]);
        assert_eq!(s.plan.as_deref(), Some("# Plan\n\n1. Do it"));
        feed(&mut s, &[tool_use("a2", "ExitPlanMode", json!({ "plan": "# Better plan" }))]);
        assert_eq!(s.plan.as_deref(), Some("# Better plan"));
        feed(&mut s, &[tool_use("a3", "ExitPlanMode", json!({ "plan": "   " }))]);
        assert_eq!(s.plan.as_deref(), Some("# Better plan"), "an empty plan does not wipe the last one");
    }

    #[test]
    fn a_codex_plan_item_is_kept_and_shown() {
        let mut s = ChatState::new(Agent::Codex, "conv");
        feed(
            &mut s,
            &[json!({ "type": "event_msg", "payload": { "type": "item_completed",
                "item": { "type": "Plan", "id": "p1", "text": "# Spec\n\nBuild the thing." } } })],
        );
        assert_eq!(s.plan.as_deref(), Some("# Spec\n\nBuild the thing."));
        assert!(matches!(&s.items[0].body, Body::Tool { tool, summary, .. } if tool == "Plan" && summary == "Proposed a plan"));
    }

    #[test]
    fn the_preview_is_the_last_message_in_one_line() {
        let mut s = claude();
        assert_eq!(s.preview(), None);
        feed(
            &mut s,
            &[
                user_text("u1", "first question"),
                assistant("a1", json!({ "type": "text", "text": "Sure.\n\nHere   is the\nanswer." })),
                assistant("a2", json!({ "type": "thinking", "thinking": "private musings" })),
                tool_use("a3", "Bash", json!({ "command": "ls" })),
            ],
        );
        assert_eq!(s.preview().as_deref(), Some("Sure. Here is the answer."), "thinking and tools are skipped");
        feed(&mut s, &[assistant("a4", json!({ "type": "text", "text": "x".repeat(500) }))]);
        let p = s.preview().unwrap();
        assert_eq!(p.chars().count(), PREVIEW_LINE + 1, "clipped with an ellipsis");
        assert!(p.ends_with('…'));
    }

    #[test]
    fn a_log_that_starts_over_forgets_the_old_todos_and_plan() {
        let mut s = claude();
        feed(&mut s, &[tool_use("a1", "ExitPlanMode", json!({ "plan": "old" })), tool_use("a2", "TodoWrite", json!({ "todos": [{ "content": "x", "status": "pending" }] }))]);
        s.reset();
        assert!(s.todos.is_empty() && s.plan.is_none());
    }

    #[test]
    fn a_codex_patch_becomes_one_edit_per_file() {
        let mut s = ChatState::new(Agent::Codex, "conv");
        feed(
            &mut s,
            &[json!({ "type": "event_msg", "payload": { "type": "item_completed", "item": {
                "type": "FileChange", "id": "fc1", "status": "completed", "stdout": "", "stderr": "",
                "changes": {
                    "/p/new.md": { "type": "add", "content": "# hi\n" },
                    "/p/old.md": { "type": "delete", "content": "bye\n" },
                    "/p/src/lib.rs": { "type": "update", "move_path": null,
                        "unified_diff": "--- a/lib.rs\n+++ b/lib.rs\n@@ -1,3 +1,3 @@\n fn a() {}\n-fn b() {}\n+fn b2() {}\n fn c() {}\n@@ -9,2 +9,3 @@\n x\n+y\n\\ No newline at end of file\n" },
                } } } })],
        );
        let tools: Vec<_> = s
            .items
            .iter()
            .filter_map(|i| match &i.body {
                Body::Tool { tool, summary, diff, status, .. } => Some((tool.as_str(), summary.as_str(), diff.clone().unwrap(), *status)),
                _ => None,
            })
            .collect();
        assert_eq!(tools.len(), 3);
        let by = |name: &str| tools.iter().find(|t| t.2.path.ends_with(name)).unwrap();
        let (t, sum, d, st) = by("new.md");
        assert_eq!((*t, sum.starts_with("Created"), *st), ("Write", true, ToolStatus::Ok));
        assert_eq!(d.edits, vec![EditPair { old: "".into(), new: "# hi\n".into() }]);
        assert_eq!((by("old.md").0, by("old.md").2.edits[0].new.as_str()), ("Delete", ""));
        let lib = by("lib.rs");
        assert_eq!((lib.0, lib.1), ("Edit", "Edited src/lib.rs"));
        assert_eq!(
            lib.2.edits,
            vec![
                EditPair { old: "fn a() {}\nfn b() {}\nfn c() {}".into(), new: "fn a() {}\nfn b2() {}\nfn c() {}".into() },
                EditPair { old: "x".into(), new: "x\ny".into() },
            ]
        );
    }

    #[test]
    fn a_declined_codex_patch_is_an_error_with_its_message() {
        let mut s = ChatState::new(Agent::Codex, "conv");
        feed(
            &mut s,
            &[json!({ "type": "event_msg", "payload": { "type": "item_completed", "item": {
                "type": "FileChange", "id": "fc2", "status": "declined", "stderr": "patch rejected",
                "changes": { "/p/a.rs": { "type": "add", "content": "x" } } } } })],
        );
        assert!(matches!(&s.items[0].body, Body::Tool { status: ToolStatus::Error, output: Some(o), .. } if o == "patch rejected"));
    }

    // ------------------------------------------------------------- helper agents

    fn side_say(uuid: &str, text: &str) -> Value {
        json!({ "type": "assistant", "uuid": uuid, "isSidechain": true, "message": { "model": "m", "content": [{ "type": "text", "text": text }] } })
    }

    #[test]
    fn a_helpers_side_chain_lines_are_ignored_by_the_conversation_but_read_from_its_own_log() {
        let mut main = claude();
        feed(&mut main, &[side_say("s1", "from a helper")]);
        assert!(main.items.is_empty(), "the main conversation does not show a helper's lines");
        let mut sub = ChatState::new_sub(Agent::Claude, PathBuf::from("/x/agent-a.jsonl"));
        feed(&mut sub, &[side_say("s1", "from a helper")]);
        assert_eq!(bodies(&sub), vec![&Body::Assistant { text: "from a helper".into() }]);
        sub.reset();
        feed(&mut sub, &[side_say("s2", "after a restart")]);
        assert_eq!(sub.items.len(), 1, "a log that starts over is still a helper's");
    }

    #[test]
    fn the_summary_counts_tools_and_keeps_the_last_action_and_report() {
        let mut s = claude();
        assert_eq!(s.summary(), Summary::default());
        feed(
            &mut s,
            &[
                assistant("a1", json!({ "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "ls", "description": "List files" } })),
                assistant("a2", json!({ "type": "text", "text": "  all done  " })),
                assistant("a3", json!({ "type": "tool_use", "id": "t2", "name": "Read", "input": { "file_path": "/p/src/x.rs" } })),
            ],
        );
        let sum = s.summary();
        assert_eq!((sum.tools, sum.last_action.as_deref(), sum.result.as_deref()), (2, Some("Read src/x.rs"), Some("all done")));
        assert!(sum.first_ts.is_some());
    }

    #[test]
    fn a_codex_helper_start_is_a_running_agent_card_that_finishes_when_the_log_says_so() {
        let mut s = ChatState::new(Agent::Codex, "conv");
        let ev = |kind: &str, id: &str, thread: &str, path: &str| {
            json!({ "type": "event_msg", "payload": { "type": "item_completed", "item": {
                "type": "SubAgentActivity", "id": id, "kind": kind, "agent_thread_id": thread, "agent_path": path } } })
        };
        feed(&mut s, &[ev("started", "call_9", "t1", "/root/diagnostic_escalation")]);
        assert!(matches!(&s.items[0].body, Body::Tool { tool, summary, status: ToolStatus::Running, .. } if tool == "Agent" && summary == "diagnostic escalation"));
        // Messages sent to a helper, and anything about the main thread, add nothing.
        feed(&mut s, &[ev("interacted", "call_10", "t1", "/root/diagnostic_escalation"), ev("started", "call_11", "t0", "/root")]);
        assert_eq!(s.items.len(), 1);
        feed(&mut s, &[ev("completed", "done", "t1", "/root/diagnostic_escalation")]);
        assert!(matches!(&s.items[0].body, Body::Tool { status: ToolStatus::Ok, .. }), "the same card, now finished");
        assert_eq!((s.spawns()[0].depth, s.spawns()[0].done), (1, true));
    }

    #[test]
    fn a_response_carries_the_helper_list_in_json() {
        let s = claude();
        let mut r = ChatResponse::of(&s, Agent::Claude, true, None, 0);
        r.agents.push(SubAgent {
            id: "a1".into(), kind: "Explore".into(), description: "Find it".into(), tool_use_id: Some("toolu_1".into()), depth: 1, parent: None,
            running: true, started: Some(5), last: Some(9), tools: 3, last_action: Some("Read x".into()), result: None, has_log: true,
        });
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["agents"][0]["id"], "a1");
        assert_eq!(v["agents"][0]["tool_use_id"], "toolu_1");
        assert_eq!(v["agents"][0]["running"], true);
    }
}
