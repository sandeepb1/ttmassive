//! Orchestrator mode: role-based agent flows that run Claude Code and Codex headless.

pub mod api;
pub mod claude;
pub mod codex;
pub mod downshift;
pub mod exec;
pub mod gate;
pub mod graph;
pub mod handoff;
pub mod prompts;
pub mod roles;
pub mod run;
pub mod runner;
pub mod service;
pub mod starters;
pub mod store;
pub mod usage;
pub mod worktree;

use serde_json::Value;

use usage::Tokens;

/// What one agent turn produced, whichever engine ran it.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The answer, matching the node's output schema.
    pub structured: Option<Value>,
    pub text: String,
    /// What this turn used.
    pub tokens: Tokens,
    pub cost_usd: f64,
    pub is_error: bool,
    /// The agent's own conversation id, so a person can later open it in a live session.
    pub session: Option<String>,
}
