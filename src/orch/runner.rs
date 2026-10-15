//! The real `NodeRunner`: starts the Claude or Codex agent a node asks for and keeps it for the rest
//! of the run, so a retry continues the same conversation and the prompt cache stays warm. Read-only
//! roles work in the project folder; a builder works in its own git worktree.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

use super::Outcome;
use super::claude::{self, Access, Event};
use super::codex;
use super::exec::NodeRunner;
use super::graph::{Engine, Flow, Node};
use super::roles::Capability;
use super::run::{SKILL_PREFIX, SUBAGENT_PREFIX};
use super::handoff;
use super::worktree;

pub struct EngineRunner {
    flow: Flow,
    project: PathBuf,
    worktrees: PathBuf,
    run_id: String,
    codex: Arc<codex::Hub>,
    claude: Mutex<HashMap<String, claude::Agent>>,
    threads: Mutex<HashMap<String, codex::Thread>>,
    /// Codex threads that are in the middle of a turn, so a cancel can stop them.
    active: std::sync::Mutex<HashSet<String>>,
    /// Builders can start together, and `git worktree add` on one repository must not overlap itself.
    worktree_lock: Mutex<()>,
}

/// One line of a transcript: the first line of `s`, cut short.
fn preview(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let mut out: String = line.chars().take(160).collect();
    if line.chars().count() > 160 {
        out.push('…');
    }
    out
}

/// What a node can do in its folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Limits {
    /// Changes files (a builder).
    Writes,
    /// Reads and searches only.
    Reads,
    /// Reads, and runs commands in a sandbox that cannot change anything.
    Shell,
}

fn limits(node: &Node) -> Limits {
    if node.writes() {
        Limits::Writes
    } else if node.capability() == Capability::Inspect || node.engine == Engine::Codex {
        // A Claude agent that may inspect is given a sandboxed shell; Codex's read-only sandbox already allows one.
        Limits::Shell
    } else {
        Limits::Reads
    }
}

/// Tells a node where it is and what it may do, so it does not wander off to a path in the brief or
/// ask itself for something its tools cannot give. Goes after the variable part of the prompt.
fn with_folder_note(prompt: &str, cwd: &std::path::Path, limits: Limits) -> String {
    let mut p = format!(
        "{prompt}\n\n# Working folder\n\nYou are working in `{}`. \"The project root\" in the brief and the plan means \
         this folder. Use paths relative to it and do not read or write outside it.",
        cwd.display()
    );
    match limits {
        Limits::Writes => {}
        Limits::Reads => p.push_str(
            " You can read files and search, but you cannot run commands or change anything. If something can \
             only be checked by running it, say so in your answer instead of failing the work for that alone.",
        ),
        Limits::Shell => p.push_str(
            " You can read files, search and run commands such as tests or `git status`, but the shell is \
             sandboxed: it cannot change any file and cannot read outside this folder. Do not try to fix \
             anything; report what you find.",
        ),
    }
    p
}

impl EngineRunner {
    pub fn new(flow: Flow, project: PathBuf, worktrees: PathBuf, run_id: String, codex: Arc<codex::Hub>) -> Self {
        Self {
            flow,
            project,
            worktrees,
            run_id,
            codex,
            claude: Mutex::default(),
            threads: Mutex::default(),
            active: std::sync::Mutex::default(),
            worktree_lock: Mutex::new(()),
        }
    }

    /// Where a builder's worktree is.
    pub fn worktree_dir(&self, node_id: &str) -> PathBuf {
        worktree::path(&self.worktrees, &self.run_id, node_id)
    }

    /// Let go of a node's headless agent, so a person can take over its conversation without two
    /// processes writing to it. A later turn on that node starts a fresh agent.
    pub async fn release(&self, node_id: &str) {
        self.claude.lock().await.remove(node_id); // dropping the agent ends its process
        self.threads.lock().await.remove(node_id);
    }

    /// Stop any Codex turn still running. Claude agents die when the turn's future is dropped.
    pub async fn interrupt(&self) {
        let ids: Vec<String> = self.active.lock().unwrap_or_else(|e| e.into_inner()).drain().collect();
        if let Ok(server) = self.codex.get().await {
            for id in ids {
                server.interrupt(&id).await;
            }
        }
    }

    /// Builders work in their own worktree. A reader that comes after a builder (a judge, say) looks
    /// at that builder's worktree, because that is where the work is; any other reader gets the project.
    async fn cwd(&self, node: &Node) -> Result<PathBuf> {
        if node.writes() {
            let _one_at_a_time = self.worktree_lock.lock().await;
            return worktree::ensure(&self.project, &self.worktrees, &self.run_id, &node.id).await;
        }
        if let Some(writer) = self.flow.nearest_writer(&node.id) {
            let dir = worktree::path(&self.worktrees, &self.run_id, writer);
            if dir.join(".git").exists() {
                return Ok(dir);
            }
        }
        Ok(self.project.clone())
    }

    async fn run_claude(&self, node: &Node, cwd: PathBuf, prompt: &str, log: &mut (dyn FnMut(String) + Send)) -> Result<Outcome> {
        let taken = self.claude.lock().await.remove(&node.id);
        let mut agent = match taken {
            Some(a) => a,
            None => {
                let spec = claude::Spec {
                    model: node.model.clone(),
                    system_prompt: node.system_prompt.clone(),
                    schema: handoff::schema_for(node.output(), &self.flow.send_back_targets(&node.id)),
                    access: match limits(node) {
                        Limits::Writes => Access::Write,
                        _ if node.capability() == Capability::Inspect => Access::Inspect,
                        _ => Access::ReadOnly,
                    },
                    cwd,
                    // A worktree keeps its history in the main repository's .git; `git status` needs it.
                    read_also: vec![self.project.join(".git")],
                };
                claude::Agent::start(&spec).await?
            }
        };
        let outcome = agent
            .turn(prompt, |ev| match ev {
                Event::Said(t) => log(preview(t)),
                Event::Tool(t) => log(format!("→ {t}")),
                Event::Skill(s) => log(format!("{SKILL_PREFIX}{s}")),
                Event::Subagent(s) => log(format!("{SUBAGENT_PREFIX}{s}")),
                Event::Nested(t) => log(format!("    ↳ {}", preview(t))),
                _ => {}
            })
            .await?;
        // Only a healthy agent is kept; after an error the next attempt starts a fresh one.
        self.claude.lock().await.insert(node.id.clone(), agent);
        Ok(outcome)
    }

    async fn run_codex(&self, node: &Node, cwd: PathBuf, prompt: &str, log: &mut (dyn FnMut(String) + Send)) -> Result<Outcome> {
        let server = self.codex.get().await?;
        let taken = self.threads.lock().await.remove(&node.id);
        let mut thread = match taken {
            Some(t) => t,
            None => {
                let spec = codex::Spec {
                    model: node.model.clone(),
                    system_prompt: node.system_prompt.clone(),
                    read_only: !node.writes(),
                    cwd,
                };
                server.start_thread(&spec).await?
            }
        };
        let schema = handoff::schema_for(node.output(), &self.flow.send_back_targets(&node.id));
        self.active.lock().unwrap_or_else(|e| e.into_inner()).insert(thread.id.clone());
        let result = server
            .turn(&mut thread, prompt, &schema, |note| {
                if let Some(line) = codex::describe(note) {
                    log(line);
                }
            })
            .await;
        self.active.lock().unwrap_or_else(|e| e.into_inner()).remove(&thread.id);
        let outcome = result?;
        self.threads.lock().await.insert(node.id.clone(), thread);
        Ok(outcome)
    }
}

impl NodeRunner for EngineRunner {
    async fn run(&self, node: &Node, prompt: &str, log: &mut (dyn FnMut(String) + Send)) -> Result<Outcome> {
        let cwd = self.cwd(node).await?;
        let prompt = with_folder_note(prompt, &cwd, limits(node));
        let folder = cwd.clone();
        let result = match node.engine {
            Engine::Claude => self.run_claude(node, cwd, &prompt, log).await,
            Engine::Codex => self.run_codex(node, cwd, &prompt, log).await,
        };
        // Whatever the builder got done is kept on its branch, even if its turn then failed: the work is not lost.
        if node.writes() {
            let msg = format!("{} (ttm run {})", node.name, self.run_id);
            match worktree::commit_all(&folder, &msg).await {
                Ok(true) => log(format!("Committed its changes to {}", worktree::branch_name(&self.run_id, &node.id))),
                Ok(false) => {}
                Err(e) => log(format!("Could not commit its changes: {e:#}")),
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orch::graph::Role;

    #[test]
    fn the_folder_note_names_the_folder_and_the_limits() {
        let dir = std::path::Path::new("/work/wt/build");
        let ro = with_folder_note("# Brief\n\nDo it.", dir, Limits::Reads);
        assert!(ro.starts_with("# Brief"), "the variable part stays first");
        assert!(ro.contains("`/work/wt/build`") && ro.contains("relative"));
        assert!(ro.contains("cannot run commands"));
        let rw = with_folder_note("x", dir, Limits::Writes);
        assert!(rw.contains("`/work/wt/build`") && !rw.contains("cannot run commands"));
        let sh = with_folder_note("x", dir, Limits::Shell);
        assert!(sh.contains("run commands") && sh.contains("sandboxed") && !sh.contains("cannot run commands"));
    }

    fn node(role: Role, engine: Engine) -> Node {
        Node {
            id: "n".into(),
            name: "n".into(),
            role,
            custom: None,
            engine,
            model: "m".into(),
            system_prompt: String::new(),
            checkpoint: false,
            token_cap: None,
            keep_model: false,
            x: 0.0,
            y: 0.0,
        }
    }

    #[test]
    fn a_judge_gets_a_shell_and_a_researcher_does_not() {
        assert_eq!(limits(&node(Role::Builder, Engine::Claude)), Limits::Writes);
        assert_eq!(limits(&node(Role::Judge, Engine::Claude)), Limits::Shell);
        assert_eq!(limits(&node(Role::Researcher, Engine::Claude)), Limits::Reads);
        assert_eq!(limits(&node(Role::Search, Engine::Claude)), Limits::Reads);
        // Codex's read-only sandbox already allows commands, so its readers are told so.
        assert_eq!(limits(&node(Role::Researcher, Engine::Codex)), Limits::Shell);
    }

    #[test]
    fn a_preview_is_one_short_line() {
        assert_eq!(preview("\n  First line\nsecond"), "First line");
        assert_eq!(preview(""), "");
        let long = "x".repeat(300);
        let p = preview(&long);
        assert_eq!(p.chars().count(), 161);
        assert!(p.ends_with('…'));
    }
}
