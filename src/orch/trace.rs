//! What an agent did in one turn, recorded by ttm itself: the prompt it was given, what it said, each tool it called and
//! what came back. Most agents leave no log of their own (only a builder's conversation is kept on disk), and a direct
//! call has none, so this is the only full record. Every piece is cut to a size, and a turn's total is capped, so a
//! chatty agent cannot fill the disk.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Longest a prompt is kept.
const MAX_PROMPT: usize = 60_000;
/// Longest a piece of what the agent said is kept.
const MAX_SAY: usize = 20_000;
/// Longest a tool's input or result is kept.
const MAX_TOOL: usize = 4_000;
/// Longest a turn's whole record is.
const MAX_TOTAL: usize = 300_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// What the agent was asked.
    Prompt,
    /// Something the agent said.
    Say,
    /// A tool the agent called.
    Tool,
    /// What a tool gave back.
    Result,
    /// Anything else worth a line: reasoning, a sub-agent, something unexpected.
    Note,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub kind: Kind,
    /// A short label: the tool's name, who said it.
    pub title: String,
    pub body: String,
}

/// Cut `s` to `max` characters, saying so when it was.
pub fn clip(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    format!("{}\n… ({} more characters)", s.chars().take(max).collect::<String>(), n - max)
}

/// A turn's entries, built up as the agent works. Stops growing at the total cap and says so once.
#[derive(Debug, Default, Clone)]
pub struct Trace {
    entries: Vec<Entry>,
    size: usize,
    full: bool,
}

impl Trace {
    pub fn push(&mut self, kind: Kind, title: impl Into<String>, body: &str) {
        if self.full {
            return;
        }
        let max = match kind {
            Kind::Prompt => MAX_PROMPT,
            Kind::Say | Kind::Note => MAX_SAY,
            Kind::Tool | Kind::Result => MAX_TOOL,
        };
        let e = Entry { kind, title: title.into(), body: clip(body.trim(), max) };
        if self.size + e.body.len() > MAX_TOTAL {
            self.full = true;
            self.entries.push(Entry { kind: Kind::Note, title: "log full".into(), body: "The rest of this turn was not recorded: it was longer than ttm keeps.".into() });
            return;
        }
        self.size += e.body.len() + e.title.len();
        self.entries.push(e);
    }

    pub fn into_entries(self) -> Vec<Entry> {
        self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// A tool's input as one compact line of JSON, or the part of it that says the most.
pub fn compact(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// The text of a tool result, which Claude sends as a string or as a list of blocks.
pub fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        other => compact(other),
    }
}

/// Everything a finished turn leaves besides its outcome.
#[derive(Debug, Default, Clone)]
pub struct TurnMeta {
    pub trace: Vec<Entry>,
    /// The commit a builder's work was saved in.
    pub commit: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pieces_are_cut_to_their_size_and_say_so() {
        let mut t = Trace::default();
        t.push(Kind::Tool, "Read", &"x".repeat(MAX_TOOL + 50));
        t.push(Kind::Say, "", "short");
        let e = t.into_entries();
        assert!(e[0].body.ends_with("… (50 more characters)") && e[0].body.chars().count() < MAX_TOOL + 40);
        assert_eq!(e[1].body, "short");
    }

    #[test]
    fn a_turn_stops_growing_at_its_total_and_says_so_once() {
        let mut t = Trace::default();
        for _ in 0..40 {
            t.push(Kind::Say, "", &"y".repeat(MAX_SAY));
        }
        let before = t.len();
        t.push(Kind::Say, "", "ignored");
        assert_eq!(t.len(), before, "nothing is added once it is full");
        let e = t.into_entries();
        assert_eq!(e.iter().filter(|e| e.title == "log full").count(), 1);
        assert!(e.iter().map(|e| e.body.len()).sum::<usize>() < MAX_TOTAL + 1000);
    }

    #[test]
    fn tool_input_and_results_become_text() {
        assert_eq!(compact(&json!({"path": "a.rs"})), "{\"path\":\"a.rs\"}");
        assert_eq!(compact(&json!("plain")), "plain");
        assert_eq!(compact(&Value::Null), "");
        assert_eq!(result_text(&json!("out")), "out");
        assert_eq!(result_text(&json!([{"type": "text", "text": "a"}, {"type": "image"}, {"type": "text", "text": "b"}])), "a\nb");
    }
}
