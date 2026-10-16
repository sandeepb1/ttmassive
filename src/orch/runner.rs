//! The real `NodeRunner`: starts the Claude or Codex agent a node asks for and keeps it for the rest
//! of the run, so a retry continues the same conversation and the prompt cache stays warm. Read-only
//! roles work in the project folder; a builder works in its own git worktree.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use tokio::sync::Mutex;

use super::trace::{Kind, Trace, TurnMeta};
use super::{Outcome, Proxy};
use crate::openrouter::proxy::Leases;
use crate::providers::chat::{self, Message};
use super::claude::{self, Access, Event};
use super::codex;
use super::exec::{Answer, Ask, NodeRunner};
use super::route::{self, Choice};
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
    /// A node's Codex thread, and the account it was started on: an agent that moves to another account needs a new one.
    threads: Mutex<HashMap<String, (codex::Thread, Option<String>)>>,
    /// Codex threads that are in the middle of a turn, so a cancel can stop them.
    active: std::sync::Mutex<HashSet<String>>,
    /// Builders can start together, and `git worktree add` on one repository must not overlap itself.
    worktree_lock: Mutex<()>,
    /// Where agents on OpenRouter get their credentials: one lease per agent, tagged `orch:<run>:<node>`.
    leases: Arc<Leases>,
    /// Where those agents reach the proxy.
    origin: String,
    /// The plan limits, for the dollar cap a new lease is given.
    limits: Arc<std::sync::Mutex<super::graph::Budget>>,
    /// The user chose to carry on past the limits, so new leases get no dollar cap.
    waived: AtomicBool,
    /// Where the accounts are kept, for agents that call their model directly.
    data_dir: PathBuf,
    /// Documentation crawled for the run, written into each builder's worktree.
    docs: Vec<super::run::Reference>,
    /// What each direct agent has said and been told so far, so a send-back carries on the same conversation.
    direct: Mutex<HashMap<String, Vec<Message>>>,
    /// The record of each agent's last turn, until the executor takes it.
    meta: std::sync::Mutex<HashMap<String, TurnMeta>>,
}

/// Said to an agent that calls its model directly: what it does not have.
const DIRECT_NOTE: &str = "# What you have\n\nYou have no tools: you cannot read files, search, browse or run anything. Work only from the brief above and what \
the earlier agents handed to you. If the work needs something you cannot see, say so in your answer instead of guessing.";

/// What to say to an agent that finished without answering in the required format: the format itself, since a model that is
/// not held to it has never seen it.
fn repair_prompt(schema: &serde_json::Value) -> String {
    format!(
        "Your last message was not in the required output format, so it could not be handed on. Do not search, read or run \
anything more, and do not repeat your report at length. Using only what you have already found, reply now with ONLY one JSON \
object that matches this JSON Schema, with no code fence and no other text. Keep every string short (one or two sentences); \
put the detail in the lists.\n\n{}",
        serde_json::to_string_pretty(schema).unwrap_or_default()
    )
}

/// Said instead of that to a direct agent that has the web tools.
const DIRECT_WEB_NOTE: &str = "# What you have\n\nYou can search the web and fetch pages with the tools you are given, a few times at most. You cannot read files or run \
anything. Otherwise work from the brief above and what the earlier agents handed to you. If the work needs something you cannot see, say so.";

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
fn with_folder_note(prompt: &str, cwd: &std::path::Path, limits: Limits, docs: bool) -> String {
    let mut p = format!(
        "{prompt}\n\n# Working folder\n\nYou are working in `{}`. \"The project root\" in the brief and the plan means \
         this folder. Use paths relative to it and do not read or write outside it.",
        cwd.display()
    );
    match limits {
        Limits::Writes if docs => p.push_str(&format!(" Reference documentation for this task is in `{}`: read it when you need it. It is not part of the project, so do not change it or add it to your work.", worktree::REFERENCE_DIR)),
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
    pub fn new(
        flow: Flow,
        project: PathBuf,
        worktrees: PathBuf,
        run_id: String,
        codex: Arc<codex::Hub>,
        leases: Arc<Leases>,
        origin: String,
        limits: Arc<std::sync::Mutex<super::graph::Budget>>,
        waived: bool,
        data_dir: PathBuf,
    ) -> Self {
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
            leases,
            origin,
            limits,
            waived: AtomicBool::new(waived),
            data_dir,
            docs: Vec::new(),
            direct: Mutex::default(),
            meta: std::sync::Mutex::default(),
        }
    }

    /// Give the runner the documentation crawled for its run.
    pub fn with_docs(mut self, docs: Vec<super::run::Reference>) -> Self {
        self.docs = docs;
        self
    }

    /// Write the crawled documentation into a builder's worktree, where it can be read. Not committed (see `worktree`).
    async fn write_docs(&self, dir: &std::path::Path) {
        for (i, d) in self.docs.iter().enumerate().filter(|(_, d)| !d.failed) {
            let path = dir.join(worktree::REFERENCE_DIR).join(format!("{}-{}.md", i + 1, docs_name(&d.url)));
            if path.exists() {
                continue;
            }
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&path, format!("<!-- Fetched from {} for this task. For reading only; it is not part of the project. -->\n\n{}\n", d.url, d.text)).await;
        }
    }

    fn lease_tag(&self, node_id: &str) -> String {
        lease_tag(&self.run_id, node_id)
    }

    /// The run goes on past its limits: leases stop capping dollars too, those already made included.
    pub fn waive(&self) {
        self.waived.store(true, Ordering::SeqCst);
        for n in &self.flow.nodes {
            self.leases.set_cap(&self.lease_tag(&n.id), None);
            self.leases.set_web_cap(&self.web_tag(&n.id), None);
        }
    }

    fn web_tag(&self, node_id: &str) -> String {
        format!("{}:web", self.lease_tag(node_id))
    }

    /// The web tools for an agent that is to have them, with a lease of its own so its credits are counted apart.
    /// `None` when the agent is not to have them or there is no web account to give them with.
    async fn web_for(&self, node: &Node) -> Option<Proxy> {
        if !node.web || node.direct {
            return None;
        }
        let accounts = crate::secrets::load(&self.data_dir).await;
        if !accounts.has_web() {
            return None;
        }
        let tag = self.web_tag(&node.id);
        let cap = if self.waived.load(Ordering::SeqCst) { None } else { self.limits.lock().unwrap_or_else(|e| e.into_inner()).run_web_usd };
        let token = match self.leases.by_tag(&tag) {
            Some((token, _)) => token,
            None => self.leases.mint("web", "web", &tag, None),
        };
        self.leases.set_web_cap(&tag, cap);
        Some(Proxy { origin: self.origin.clone(), token })
    }

    /// Provider credits, and their cost in dollars, that the agent's web lease has used so far.
    fn web_spent(&self, node: &Node) -> (u64, f64) {
        self.leases.by_tag(&self.web_tag(&node.id)).map_or((0, 0.0), |(_, l)| (l.spent.credits, l.spent.web_usd))
    }

    /// End the leases of every agent of this run. Called when the run is over.
    pub fn end_leases(&self) {
        self.leases.revoke_prefix(&format!("orch:{}:", self.run_id));
    }

    /// What an agent on OpenRouter is given: the proxy and its lease. The lease is kept for the life of the run,
    /// so a send-back carries on with the same spend and the same model. `None` for an agent on your own plan.
    fn proxy_for(&self, node: &Node) -> Option<Proxy> {
        let account = node.account.as_deref()?;
        let tag = self.lease_tag(&node.id);
        let cap = if self.waived.load(Ordering::SeqCst) { None } else { self.limits.lock().unwrap_or_else(|e| e.into_inner()).run_usd };
        let token = match self.leases.by_tag(&tag) {
            Some((token, lease)) => {
                if lease.model != node.model {
                    self.leases.set_model(&tag, &node.model);
                }
                self.leases.set_cap(&tag, cap);
                token
            }
            None => self.leases.mint(account, &node.model, &tag, cap),
        };
        Some(Proxy { origin: self.origin.clone(), token })
    }

    /// Dollars the node's lease has been charged so far.
    fn spent(&self, node: &Node) -> f64 {
        self.leases.by_tag(&self.lease_tag(&node.id)).map_or(0.0, |(_, l)| l.spent.cost_usd)
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
            let dir = worktree::ensure(&self.project, &self.worktrees, &self.run_id, &node.id).await?;
            self.write_docs(&dir).await;
            return Ok(dir);
        }
        if let Some(writer) = self.flow.nearest_writer(&node.id) {
            let dir = worktree::path(&self.worktrees, &self.run_id, writer);
            if dir.join(".git").exists() {
                return Ok(dir);
            }
        }
        Ok(self.project.clone())
    }

    /// One turn of an agent that calls its model itself: the conversation so far, plus this prompt.
    async fn run_direct(&self, node: &Node, prompt: &str, log: &mut (dyn FnMut(String) + Send), trace: &mut Trace) -> Result<Outcome> {
        let accounts = crate::secrets::load(&self.data_dir).await;
        let account = node.account.as_deref().and_then(|id| accounts.usable(id)).ok_or_else(|| anyhow::anyhow!("{} has no account it can use", node.name))?;
        let endpoint = chat::endpoint_for(account, &node.model);
        let schema = handoff::schema_for(node.output(), &self.flow.send_back_targets(&node.id));
        let mut history = self.direct.lock().await.get(&node.id).cloned().unwrap_or_else(|| vec![Message::system(node.system_prompt.clone())]);
        history.push(Message::user(prompt));
        log(format!("→ asked {} directly", node.model));
        // With web access it may search and fetch pages (a few calls a turn, and nothing else: no files, no shell).
        let web = if node.web { Some(crate::web::pool::Pool::of(&accounts)).filter(|p| !p.is_empty()) } else { None };
        let (reply, credits, web_usd) = match web {
            Some(pool) => {
                let tools = crate::web::tools::chat_tools(&pool);
                let r = chat::complete_with_tools(&endpoint, &history, Some(&schema), &tools, chat::MAX_TOOL_CALLS, &crate::web::tools::Web(pool)).await?;
                for e in &r.events {
                    log(format!("→ used {}", e.name));
                    trace.push(Kind::Tool, e.name.clone(), &e.arguments);
                    trace.push(Kind::Result, "result", &e.result);
                }
                history = r.messages;
                (r.reply, r.credits, r.web_usd)
            }
            None => {
                let r = chat::complete(&endpoint, &history, Some(&schema)).await?;
                history.push(Message::assistant(r.text.clone()));
                (r, 0, 0.0)
            }
        };
        trace.push(Kind::Say, node.model.clone(), &reply.text);
        self.direct.lock().await.insert(node.id.clone(), history);
        Ok(Outcome { structured: reply.structured, text: reply.text, tokens: reply.tokens, cost_usd: reply.cost_usd.unwrap_or(0.0), credits, web_usd, is_error: false, session: None })
    }

    async fn run_claude(&self, node: &Node, cwd: PathBuf, prompt: &str, log: &mut (dyn FnMut(String) + Send), trace: &mut Trace) -> Result<Outcome> {
        let taken = self.claude.lock().await.remove(&node.id);
        let spec = |resume: Option<String>| claude::Spec {
            model: node.model.clone(),
            system_prompt: node.system_prompt.clone(),
            schema: handoff::schema_for(node.output(), &self.flow.send_back_targets(&node.id)),
            access: match limits(node) {
                Limits::Writes => Access::Write,
                _ if node.capability() == Capability::Inspect => Access::Inspect,
                _ => Access::ReadOnly,
            },
            cwd: cwd.clone(),
            // A worktree keeps its history in the main repository's .git; `git status` needs it.
            read_also: vec![self.project.join(".git")],
            proxy: self.proxy_for(node),
            web: None,
            effort: node.effort.clone(),
            resume,
            // An agent that may move between models keeps its conversation, so the next model can carry on from it.
            persist: node.routing != super::graph::Routing::Pinned,
        };
        let want = claude::Identity { model: node.model.clone(), effort: node.effort.clone(), account: node.account.clone() };
        let mut resumed = false;
        let mut agent = match taken {
            Some(a) if *a.identity() == want => a,
            // The model, effort or account changed: the process cannot change them, so a new one takes over the same conversation.
            Some(old) if node.routing != super::graph::Routing::Pinned && old.session().is_some() => {
                let session = old.session().map(String::from);
                drop(old);
                log(format!("→ carrying on the conversation on {}", node.model));
                resumed = true;
                let mut s = spec(session);
                s.web = self.web_for(node).await;
                claude::Agent::start_as(&s, want.clone()).await?
            }
            _ => {
                let mut s = spec(None);
                s.web = self.web_for(node).await;
                claude::Agent::start_as(&s, want.clone()).await?
            }
        };
        let mut outcome = agent
            .turn_traced(prompt, |ev| match ev {
                Event::Said(t) => log(preview(t)),
                Event::Tool(t) => log(format!("→ {t}")),
                Event::Skill(s) => log(format!("{SKILL_PREFIX}{s}")),
                Event::Subagent(s) => log(format!("{SUBAGENT_PREFIX}{s}")),
                Event::Nested(t) => log(format!("    ↳ {}", preview(t))),
                _ => {}
            }, trace)
            .await;
        // A conversation that would not carry over to the new model (its reasoning is tied to the one that wrote it, say)
        // starts again instead of failing: the prompt holds everything the agent needs to do its part.
        if resumed && outcome.as_ref().map_or(true, |o| o.is_error) {
            log("→ the conversation would not carry over: starting it again".into());
            let mut s = spec(None);
            s.web = self.web_for(node).await;
            agent = claude::Agent::start_as(&s, want).await?;
            outcome = agent.turn_traced(prompt, |_| {}, trace).await;
        }
        let outcome = outcome?;
        // Only a healthy agent is kept; after an error the next attempt starts a fresh one.
        self.claude.lock().await.insert(node.id.clone(), agent);
        Ok(outcome)
    }

    async fn run_codex(&self, node: &Node, cwd: PathBuf, prompt: &str, log: &mut (dyn FnMut(String) + Send), trace: &mut Trace) -> Result<Outcome> {
        let server = self.codex.get().await?;
        let taken = self.threads.lock().await.remove(&node.id);
        let mut thread = match taken {
            // The thread's model and effort are changed with the turn, so it keeps its conversation. Its account is part of how
            // it was started, so an agent that moves to another one begins again.
            Some((t, account)) if account == node.account => t,
            _ => {
                let spec = codex::Spec {
                    model: node.model.clone(),
                    system_prompt: node.system_prompt.clone(),
                    read_only: !node.writes(),
                    cwd,
                    proxy: self.proxy_for(node),
                    web: self.web_for(node).await,
                };
                server.start_thread(&spec).await?
            }
        };
        let schema = handoff::schema_for(node.output(), &self.flow.send_back_targets(&node.id));
        self.active.lock().unwrap_or_else(|e| e.into_inner()).insert(thread.id.clone());
        let result = server
            .turn(&mut thread, prompt, &schema, (Some(node.model.as_str()), node.effort.as_deref()), |note| {
                codex::trace_note(note, trace);
                if let Some(line) = codex::describe(note) {
                    log(line);
                }
            })
            .await;
        self.active.lock().unwrap_or_else(|e| e.into_inner()).remove(&thread.id);
        let outcome = result?;
        self.threads.lock().await.insert(node.id.clone(), (thread, node.account.clone()));
        Ok(outcome)
    }
}

/// A file name for a documentation address: `docs-rs-serde-latest`.
pub fn docs_name(url: &str) -> String {
    let bare = url.trim_start_matches("https://").trim_start_matches("http://");
    let s: String = bare.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    s.chars().take(60).collect::<String>().trim_end_matches('-').to_owned()
}

/// The lease tag of one agent of a run.
pub fn lease_tag(run_id: &str, node_id: &str) -> String {
    format!("orch:{run_id}:{node_id}")
}

impl NodeRunner for EngineRunner {
    async fn run(&self, node: &Node, prompt: &str, log: &mut (dyn FnMut(String) + Send)) -> Result<Outcome> {
        let mut trace = Trace::default();
        let mut commit = None;
        let result = self.run_turn(node, prompt, log, &mut trace, &mut commit).await;
        self.meta.lock().unwrap_or_else(|e| e.into_inner()).insert(node.id.clone(), TurnMeta { trace: trace.into_entries(), commit });
        result
    }

    /// Ask a small model which rung of the agent's ladder fits this turn. Whatever it says is checked against the ladder by
    /// the caller; if asking fails, the rules' choice stands.
    async fn choose(&self, node: &Node, rules: &Choice, ask: &Ask) -> Option<Answer> {
        let accounts = crate::secrets::load(&self.data_dir).await;
        let prices: Vec<Option<(f64, f64)>> = node
            .ladder
            .iter()
            .map(|r| {
                let m = accounts.get(r.account.as_deref()?)?.models.iter().find(|m| m.id == r.model)?;
                Some((m.price_in?, m.price_out?))
            })
            .collect();
        let prompt = route::classifier_prompt(node, &ask.role, &ask.brief, &ask.situation, ask.feedback.as_deref(), ask.spent_usd, ask.cap_usd, &prices, crate::openrouter::intel::notes(&self.data_dir, &accounts).await.as_deref());
        let via = accounts.helper_endpoint("route");
        let scratch = self.data_dir.join("orch").join("scratch");
        let called = super::prompts::helper_call(via.as_ref(), "haiku", route::CLASSIFIER_SYSTEM, &prompt, route::classifier_schema(node.ladder.len()), &scratch).await;
        match called {
            Ok((v, outcome)) => Some(Answer { pick: route::parse_pick(&v)?, tokens: outcome.tokens, cost_usd: if via.is_some() { outcome.cost_usd } else { 0.0 } }),
            Err(e) => {
                tracing::warn!("choosing a model for {}: {e:#}; using the rules' choice (rung {})", node.name, rules.index + 1);
                None
            }
        }
    }

    fn take_meta(&self, node_id: &str) -> TurnMeta {
        self.meta.lock().unwrap_or_else(|e| e.into_inner()).remove(node_id).unwrap_or_default()
    }
}

impl EngineRunner {
    /// One turn of a node, recording what happened into `trace` (and the commit a builder's work was saved in) even if it fails.
    async fn run_turn(&self, node: &Node, prompt: &str, log: &mut (dyn FnMut(String) + Send), trace: &mut Trace, commit: &mut Option<String>) -> Result<Outcome> {
        if node.direct {
            let has_web = node.web && crate::secrets::load(&self.data_dir).await.has_web();
            let prompt = format!("{prompt}\n\n{}", if has_web { DIRECT_WEB_NOTE } else { DIRECT_NOTE });
            trace.push(Kind::Prompt, "prompt", &prompt);
            return self.run_direct(node, &prompt, log, trace).await;
        }
        let cwd = self.cwd(node).await?;
        let prompt = with_folder_note(prompt, &cwd, limits(node), node.writes() && self.docs.iter().any(|d| !d.failed));
        trace.push(Kind::Prompt, "prompt", &prompt);
        let folder = cwd.clone();
        let before = self.spent(node);
        let web_before = self.web_spent(node);
        let mut result = match node.engine {
            Engine::Claude => self.run_claude(node, cwd, &prompt, log, trace).await,
            Engine::Codex => self.run_codex(node, cwd, &prompt, log, trace).await,
        };
        // A model that is not held to the output format (the ones on other providers often are not) may do the work and then
        // answer in its own words, or in a code fence, or in a layout of its own. The right answer may already be in what it
        // wrote; if not, it still has the work in its conversation, so show it the layout and ask once before failing a turn
        // that can take minutes and cost real money.
        let schema = handoff::schema_for(node.output(), &self.flow.send_back_targets(&node.id));
        if let Ok(first) = &mut result {
            if first.structured.is_none() && !first.is_error {
                first.structured = handoff::salvage(&schema, &first.text);
            }
        }
        if let Ok(first) = &result {
            if first.structured.is_none() && !first.is_error {
                log("→ its answer was not in the required shape: asking once more for just the answer".into());
                let ask = repair_prompt(&schema);
                trace.push(Kind::Prompt, "prompt", &ask);
                let cwd = self.cwd(node).await?;
                let again = match node.engine {
                    Engine::Claude => self.run_claude(node, cwd, &ask, log, trace).await,
                    Engine::Codex => self.run_codex(node, cwd, &ask, log, trace).await,
                };
                if let (Ok(first), Ok(mut second)) = (&result, again) {
                    second.tokens.add(&first.tokens);
                    second.cost_usd += first.cost_usd;
                    second.credits += first.credits;
                    second.web_usd += first.web_usd;
                    second.session = second.session.or_else(|| first.session.clone());
                    if second.structured.is_none() && !second.is_error {
                        second.structured = handoff::salvage(&schema, &second.text);
                    }
                    result = Ok(second);
                }
            }
        }
        // On OpenRouter the charge is exact, and it is what the proxy saw: the agent's own estimate is not used.
        if node.account.is_some() {
            let charged = (self.spent(node) - before).max(0.0);
            if let Ok(o) = &mut result {
                o.cost_usd = charged;
            }
        }
        if let Ok(o) = &mut result {
            let now = self.web_spent(node);
            o.credits = now.0.saturating_sub(web_before.0);
            o.web_usd = (now.1 - web_before.1).max(0.0);
        }
        // Whatever the builder got done is kept on its branch, even if its turn then failed: the work is not lost.
        if node.writes() {
            let msg = format!("{} (ttm run {})", node.name, self.run_id);
            match worktree::commit_all(&folder, &msg).await {
                Ok(true) => {
                    log(format!("Committed its changes to {}", worktree::branch_name(&self.run_id, &node.id)));
                    *commit = worktree::head(&folder).await;
                }
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
        let ro = with_folder_note("# Brief\n\nDo it.", dir, Limits::Reads, false);
        assert!(ro.starts_with("# Brief"), "the variable part stays first");
        assert!(ro.contains("`/work/wt/build`") && ro.contains("relative"));
        assert!(ro.contains("cannot run commands"));
        let rw = with_folder_note("x", dir, Limits::Writes, false);
        assert!(rw.contains("`/work/wt/build`") && !rw.contains("cannot run commands"));
        let sh = with_folder_note("x", dir, Limits::Shell, false);
        let with_docs = with_folder_note("x", dir, Limits::Writes, true);
        assert!(with_docs.contains("docs/_ref") && with_docs.contains("do not change it"), "{with_docs}");
        assert!(!rw.contains("docs/_ref"), "only when there are docs");
        assert!(sh.contains("run commands") && sh.contains("sandboxed") && !sh.contains("cannot run commands"));
    }

    /// A runner whose accounts file holds one OpenAI-compatible account pointing at `base`.
    fn runner_with_account(base: &str, nodes: Vec<Node>) -> (EngineRunner, PathBuf) {
        let dir = std::env::temp_dir().join(format!("ttm-direct-{}-{}", std::process::id(), base.len() + nodes.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let account = crate::secrets::Account {
            id: "acct".into(),
            name: "Plain".into(),
            kind: crate::secrets::Kind::OpenaiCompatible,
            base_url: base.into(),
            key: "k".into(),
            use_in_orchestrator: true,
            models: vec![crate::secrets::PinnedModel { id: "vendor/think".into(), price_in: Some(1.0), price_out: Some(2.0), tools: false }],
            prefs: Default::default(),
            usd_per_credit: None,
        };
        let accounts = crate::secrets::Accounts { accounts: vec![account], ..Default::default() };
        std::fs::write(dir.join("accounts.json"), serde_json::to_vec(&accounts).unwrap()).unwrap();
        let flow = Flow { id: "f".into(), name: "f".into(), nodes, edges: vec![], max_retries: 1, budget: Default::default(), downshift: false, dial: 2 };
        let limits = Arc::new(std::sync::Mutex::new(crate::orch::graph::Budget::default()));
        let r = EngineRunner::new(flow, dir.clone(), dir.join("wt"), "r1".into(), Arc::default(), Arc::new(Leases::new()), "http://127.0.0.1:1".into(), limits, false, dir.clone());
        (r, dir)
    }

    #[tokio::test]
    async fn an_agent_that_calls_its_model_directly_keeps_its_conversation_and_is_charged_by_price() {
        use axum::{Router, extract::State, routing::post};
        let seen: Arc<std::sync::Mutex<Vec<serde_json::Value>>> = Arc::default();
        let app = Router::new()
            .route(
                "/v1/chat/completions",
                post(|State(seen): State<Arc<std::sync::Mutex<Vec<serde_json::Value>>>>, axum::Json(b): axum::Json<serde_json::Value>| async move {
                    seen.lock().unwrap().push(b);
                    axum::Json(serde_json::json!({"choices": [{"message": {"content": "{\"summary\":\"s\",\"steps\":[\"a\"]}"}}], "usage": {"prompt_tokens": 1000, "completion_tokens": 500}}))
                }),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut n = node(Role::Orchestrator, Engine::Claude);
        n.account = Some("acct".into());
        n.direct = true;
        n.model = "vendor/think".into();
        n.system_prompt = "You plan.".into();
        let (runner, dir) = runner_with_account(&base, vec![n.clone()]);
        let mut lines: Vec<String> = Vec::new();
        let first = runner.run(&n, "Plan the thing.", &mut |l| lines.push(l)).await.unwrap();
        assert_eq!(first.structured.as_ref().and_then(|v| v["summary"].as_str()), Some("s"));
        assert!((first.cost_usd - 0.002).abs() < 1e-12, "1000 in at $1 and 500 out at $2 per million");
        assert!(lines.iter().any(|l| l.contains("directly")));
        // A send-back: the same conversation, with its first exchange in it.
        runner.run(&n, "Try again, with feedback.", &mut |_| {}).await.unwrap();
        let calls = seen.lock().unwrap();
        let roles = |i: usize| calls[i]["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(roles(0), ["system", "user"]);
        assert_eq!(roles(1), ["system", "user", "assistant", "user"], "the second turn carries the first");
        assert_eq!(calls[0]["messages"][0]["content"], "You plan.");
        assert!(calls[0]["messages"][1]["content"].as_str().unwrap().contains("You have no tools"), "told what it does not have");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_follow_up_shows_the_agent_the_exact_layout_and_asks_for_nothing_else() {
        let schema = handoff::schema_for(crate::orch::roles::Output::Report, &[]);
        let p = repair_prompt(&schema);
        assert!(p.contains("\"summary\"") && p.contains("\"findings\"") && p.contains("\"sources\""), "{p}");
        assert!(p.contains("ONLY one JSON object") && p.contains("no code fence") && p.contains("Do not search"));
    }

    #[test]
    fn docs_get_readable_file_names() {
        assert_eq!(docs_name("https://docs.rs/serde/latest/serde/"), "docs-rs-serde-latest-serde");
        assert_eq!(docs_name("http://Example.com/A_B?x=1"), "example-com-a-b-x-1");
        assert!(docs_name(&format!("https://x.test/{}", "a".repeat(200))).len() <= 60);
    }

    #[tokio::test]
    async fn crawled_docs_are_written_once_into_a_builders_folder_and_failed_ones_are_skipped() {
        let dir = std::env::temp_dir().join(format!("ttm-docs-{}", std::process::id()));
        let (runner, _) = runner_with_account("http://127.0.0.1:1/v1", vec![]);
        let runner = runner.with_docs(vec![
            crate::orch::run::Reference { url: "https://a.test/docs".into(), text: "Doc body".into(), credits: 3, web_usd: 0.0, failed: false },
            crate::orch::run::Reference { url: "https://b.test".into(), text: "failed".into(), credits: 0, web_usd: 0.0, failed: true },
        ]);
        runner.write_docs(&dir).await;
        let f = dir.join("docs/_ref/1-a-test-docs.md");
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("Doc body") && text.contains("Fetched from https://a.test/docs") && text.contains("not part of the project"));
        assert_eq!(std::fs::read_dir(dir.join("docs/_ref")).unwrap().count(), 1, "the failed one has no file");
        std::fs::write(&f, "edited").unwrap();
        runner.write_docs(&dir).await;
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "edited", "an existing file is not overwritten");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_direct_agent_whose_account_cannot_be_used_fails_with_a_reason() {
        let mut n = node(Role::Orchestrator, Engine::Claude);
        n.account = Some("gone".into());
        n.direct = true;
        let (runner, dir) = runner_with_account("http://127.0.0.1:1/v1", vec![n.clone()]);
        let err = runner.run(&n, "x", &mut |_| {}).await.unwrap_err().to_string();
        assert!(err.contains("no account it can use"), "{err}");
        std::fs::remove_dir_all(dir).ok();
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
            account: None,
            direct: false,
            web: false,
            effort: None,
            routing: crate::orch::graph::Routing::Pinned,
            ladder: Vec::new(),
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
