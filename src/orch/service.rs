//! Everything the API calls: create a run, grill, start, approve, resume, cancel. It owns the live
//! runs (the shared state the executor works on and the agents each run keeps), saves every change
//! and announces it on a channel the page listens to.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;
use tokio::sync::{Notify, broadcast};

use crate::AppState;

use super::codex;
use super::exec::{self, Control};
use super::gate::{self, Windows};
use super::graph::Flow;
use super::prompts::{self, GrillStep};
use super::run::{Grill, NodeStatus, Pause, Qa, Run, RunStatus};
use super::runner::EngineRunner;
use super::store::Store;

/// How often the plan-limit windows are re-read while a run is going.
const WINDOW_REFRESH: Duration = Duration::from_secs(30);
const FALLBACK_CODEX: [&str; 2] = ["gpt-6.1-sol", "gpt-6-luna"];

/// Why a request could not be done, in terms the API can turn into a status code.
#[derive(Debug)]
pub enum Fail {
    NotFound,
    Bad(String),
    Conflict(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for Fail {
    fn from(e: anyhow::Error) -> Self {
        Fail::Internal(e)
    }
}

/// One run that is loaded: the state the executor edits, and what it needs to keep going.
struct Live {
    shared: Mutex<Run>,
    runner: EngineRunner,
    cancelled: AtomicBool,
    cancel: Notify,
    /// A drive loop is going; stops two from running at once.
    driving: AtomicBool,
    /// A grilling call is going.
    grilling: AtomicBool,
}

pub struct Service {
    pub store: Store,
    codex: Arc<codex::Hub>,
    runs: Mutex<HashMap<String, Arc<Live>>>,
    /// (run id, the run as JSON) after every change.
    events: broadcast::Sender<(String, Arc<String>)>,
    windows: Arc<Mutex<Windows>>,
    /// The plan limits. Read each time a run is checked, so a change reaches runs that are already going.
    limits: Arc<Mutex<super::graph::Budget>>,
    scratch: PathBuf,
    data_dir: PathBuf,
    /// Credentials for agents on OpenRouter, and where they reach the proxy.
    leases: Arc<crate::openrouter::proxy::Leases>,
    origin: String,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn new_id() -> String {
    let mut b = [0u8; 3];
    rand::fill(&mut b[..]);
    format!("r{:x}{:02x}{:02x}{:02x}", now(), b[0], b[1], b[2])
}

impl Service {
    pub fn new(data_dir: &Path, leases: Arc<crate::openrouter::proxy::Leases>, origin: String) -> Service {
        let store = Store::new(data_dir);
        // A run that was going when ttm stopped did not finish; say so rather than show it as running.
        for mut r in store.all_runs() {
            if r.status == RunStatus::Running {
                r.mark_interrupted();
                if let Err(e) = store.save_run(&r) {
                    tracing::warn!("orchestrator: marking run {} interrupted: {e:#}", r.id);
                }
            }
        }
        Service {
            scratch: data_dir.join("orch").join("scratch"),
            data_dir: data_dir.to_path_buf(),
            limits: Arc::new(Mutex::new(store.load_limits())),
            store,
            codex: Arc::default(),
            runs: Mutex::default(),
            events: broadcast::channel(256).0,
            windows: Arc::default(),
            leases,
            origin,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, Arc<String>)> {
        self.events.subscribe()
    }

    fn persist(&self, r: &Run) {
        if let Err(e) = self.store.save_run(r) {
            tracing::warn!("orchestrator: saving run {}: {e:#}", r.id);
        }
        if let Ok(json) = serde_json::to_string(r) {
            let _ = self.events.send((r.id.clone(), Arc::new(json)));
        }
    }

    async fn refresh_windows(&self, st: &Arc<AppState>) -> Windows {
        let w = Windows::from_snapshot(&crate::usage::snapshot(st, false).await);
        *self.windows.lock().unwrap_or_else(|e| e.into_inner()) = w;
        w
    }

    /// The loaded run, reading it from disk the first time.
    fn live(&self, id: &str) -> Result<Arc<Live>, Fail> {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = runs.get(id) {
            return Ok(l.clone());
        }
        let run = self.store.load_run(id).ok_or(Fail::NotFound)?;
        let docs = run.docs.clone();
        let live = Arc::new(Live {
            runner: EngineRunner::new(
                run.flow.clone(),
                PathBuf::from(&run.project),
                self.store.worktrees(),
                run.id.clone(),
                self.codex.clone(),
                self.leases.clone(),
                self.origin.clone(),
                self.limits.clone(),
                run.waived,
                self.data_dir.clone(),
            )
            .with_docs(docs),
            shared: Mutex::new(run),
            cancelled: AtomicBool::new(false),
            cancel: Notify::new(),
            driving: AtomicBool::new(false),
            grilling: AtomicBool::new(false),
        });
        runs.insert(id.to_string(), live.clone());
        Ok(live)
    }

    /// The full record of one turn of a run.
    pub fn turn(&self, id: &str, n: u32) -> Result<super::run::TurnDetail, Fail> {
        self.live(id)?;
        self.store.read_turn(id, n).ok_or(Fail::NotFound)
    }

    pub fn get(&self, id: &str) -> Result<Run, Fail> {
        Ok(self.live(id)?.shared.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// Models each engine offers. Codex's are asked of Codex itself, so they never go stale.
    pub async fn models(&self, data_dir: &Path) -> serde_json::Value {
        let codex = self.codex_models().await;
        let accounts = crate::secrets::load(data_dir).await;
        let openrouter = crate::openrouter::launch::flow_models(&accounts);
        // `web`: agents can be given web access, because there is a Firecrawl account to give it with.
        json!({"claude": prompts::CLAUDE_MODELS, "codex": codex, "openrouter": openrouter, "web": accounts.firecrawl().is_some()})
    }

    async fn codex_models(&self) -> Vec<String> {
        let codex = match self.codex.get().await {
            Ok(s) => s.models().await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        if codex.is_empty() { FALLBACK_CODEX.iter().map(|s| s.to_string()).collect() } else { codex }
    }

    /// The model the user chose for one of ttm's own calls, if any. Read each time, so a change applies at once.
    async fn via(&self, helper: &str) -> Option<crate::providers::chat::Endpoint> {
        crate::secrets::load(&self.data_dir).await.helper_endpoint(helper)
    }

    /// The OpenRouter models a designer may give agents: the user's pinned ones on accounts allowed in flows.
    async fn or_models(&self, st: &AppState) -> Vec<crate::openrouter::launch::FlowModel> {
        // Designers pick among models an agent CLI can run; a direct call has no tools, so they leave those to the user.
        crate::openrouter::launch::flow_models(&crate::secrets::load(&st.data_dir).await).into_iter().filter(|m| !m.direct).collect()
    }

    /// A flow for a goal, and a line for each agent that was given an OpenRouter model, saying why.
    pub async fn suggest_flow(&self, st: &Arc<AppState>, goal: &str) -> Result<(Flow, Vec<String>), Fail> {
        if goal.trim().is_empty() {
            return Err(Fail::Bad("say what you want done".into()));
        }
        let models = self.codex_models().await;
        let id = format!("f{}", new_id());
        let (flow, notes, _) = prompts::suggest_flow(goal, &id, &models, &self.or_models(st).await, self.via("suggest").await.as_ref(), &self.scratch).await?;
        Ok((flow, notes))
    }

    /// Change a flow as the user asks. Returns the changed flow and what was done; nothing is saved.
    pub async fn edit_flow(&self, st: &Arc<AppState>, flow: Flow, request: &str, engine: super::graph::Engine) -> Result<(Flow, String), Fail> {
        if request.trim().is_empty() {
            return Err(Fail::Bad("say what to change".into()));
        }
        if request.len() > 2000 {
            return Err(Fail::Bad("that request is too long".into()));
        }
        let models = self.codex_models().await;
        if engine == super::graph::Engine::Codex && models.is_empty() {
            return Err(Fail::Bad("Codex has no model available right now".into()));
        }
        let (flow, summary, _) = prompts::edit_flow(&flow, request, engine, &models, &self.or_models(st).await, self.via("edit").await.as_ref(), &self.codex, &self.scratch).await?;
        Ok((flow, summary))
    }

    pub async fn node_prompt(&self, label: &str, capability: super::roles::Capability, output: super::roles::Output, name: &str, goal: &str) -> Result<String, Fail> {
        Ok(prompts::node_prompt(label, capability, output, name, goal, self.via("nodePrompt").await.as_ref(), &self.scratch).await?.0)
    }

    /// Design a role from a description: its name, what it may do, what it hands back, and its instructions.
    pub async fn custom_role(&self, description: &str) -> Result<prompts::NewRole, Fail> {
        if description.trim().len() < 8 {
            return Err(Fail::Bad("describe the role in a sentence or two".into()));
        }
        if description.len() > 2000 {
            return Err(Fail::Bad("that description is too long".into()));
        }
        Ok(prompts::custom_role(description, self.via("customRole").await.as_ref(), &self.scratch).await?.0)
    }

    /// Fetch the pages the goal links to, for every agent to read. A page that cannot be fetched is noted and the run goes on
    /// without it; so does a run with no web account.
    async fn references(&self, accounts: &crate::secrets::Accounts, goal: &str) -> Vec<super::run::Reference> {
        let links = crate::firecrawl::tools::links_in(goal);
        let Some(account) = accounts.firecrawl().filter(|_| !links.is_empty()) else { return Vec::new() };
        let Ok(client) = crate::firecrawl::client::Client::new(account) else { return Vec::new() };
        let mut out = Vec::new();
        for url in links {
            out.push(match client.scrape(&url).await {
                Ok(page) => super::run::Reference { url, text: crate::orch::trace::clip(&page.text, super::run::MAX_REFERENCE), credits: page.credits, failed: false },
                Err(e) => super::run::Reference { url, text: format!("{e:#}"), credits: 0, failed: true },
            });
        }
        out
    }

    /// Crawl the documentation sites the user named, for the builders to read. Only addresses a stranger could reach are fetched.
    async fn docs(&self, accounts: &crate::secrets::Accounts, urls: &[String]) -> Vec<super::run::Reference> {
        let Some(account) = accounts.firecrawl().filter(|_| !urls.is_empty()) else { return Vec::new() };
        let Ok(client) = crate::firecrawl::client::Client::new(account) else { return Vec::new() };
        let mut out = Vec::new();
        for url in urls.iter().map(|u| u.trim()).filter(|u| !u.is_empty()).take(crate::firecrawl::tools::MAX_LINKS) {
            out.push(match crate::firecrawl::tools::url_ok(url) {
                Err(why) => super::run::Reference { url: url.into(), text: why, credits: 0, failed: true },
                Ok(()) => match client.crawl_pages(url, crate::firecrawl::client::DOCS_PAGES).await {
                    Ok(page) => super::run::Reference { url: url.into(), text: crate::orch::trace::clip(&page.text, super::run::MAX_DOCS), credits: page.credits, failed: false },
                    Err(e) => super::run::Reference { url: url.into(), text: format!("{e:#}"), credits: 0, failed: true },
                },
            });
        }
        out
    }

    pub async fn create_run(
        &self,
        st: &Arc<AppState>,
        flow: Flow,
        goal: String,
        project: &str,
        skip_grill: bool,
        read_links: bool,
        docs: Vec<String>,
        brief: Option<String>,
    ) -> Result<Run, Fail> {
        flow.validate().map_err(Fail::Bad)?;
        // An agent on OpenRouter needs its account to be there and allowed, so say so now rather than mid-run.
        let accounts = crate::secrets::load(&st.data_dir).await;
        crate::openrouter::launch::check_accounts(
            &accounts,
            flow.nodes.iter().filter_map(|n| n.account.as_deref().map(|a| (n.name.as_str(), a, n.model.as_str(), n.direct))),
        )
        .map_err(Fail::Bad)?;
        if goal.trim().is_empty() {
            return Err(Fail::Bad("say what you want done".into()));
        }
        let project = std::fs::canonicalize(project).map_err(|_| Fail::Bad("that project folder does not exist".into()))?;
        if !project.is_dir() {
            return Err(Fail::Bad("the project must be a folder".into()));
        }
        // Builders work on copies made from the latest commit, so say so now rather than when one reaches its turn.
        if flow.nodes.iter().any(|n| n.writes()) && super::worktree::repo_state(&project).await != super::worktree::RepoState::Ready {
            return Err(Fail::Bad(
                "agents that change files need a git repository with at least one commit to work on copies of. Choose \"Set up git here\" in the run dialog, or run `git init` and make a first commit".into(),
            ));
        }
        let flow_writes = flow.nodes.iter().any(|n| n.writes());
        let w = self.refresh_windows(st).await;
        let mut run = Run::new(new_id(), flow, goal.trim().to_string(), now());
        run.project = project.to_string_lossy().into_owned();
        run.baseline = w.baseline();
        if flow_writes {
            run.docs = self.docs(&accounts, &docs).await;
            let notes: Vec<String> = run
                .docs
                .iter()
                .map(|d| if d.failed { format!("Could not crawl {}: {}", d.url, d.text) } else { format!("Crawled {} into docs/_ref ({} credit{})", d.url, d.credits, if d.credits == 1 { "" } else { "s" }) })
                .collect();
            for what in notes {
                run.note(None, super::run::LogKind::Event, what, None);
            }
        }
        if read_links {
            run.references = self.references(&accounts, goal.trim()).await;
            let notes: Vec<String> = run
                .references
                .iter()
                .map(|r| if r.failed { format!("Could not read {}: {}", r.url, r.text) } else { format!("Read {} ({} credit{})", r.url, r.credits, if r.credits == 1 { "" } else { "s" }) })
                .collect();
            for what in notes {
                run.note(None, super::run::LogKind::Event, what, None);
            }
        }
        run.status = RunStatus::Grilling;
        run.grill = Some(Grill {
            brief: skip_grill.then(|| brief.as_deref().map(str::trim).filter(|b| !b.is_empty()).unwrap_or_else(|| goal.trim()).to_string()),
            ..Grill::default()
        });
        self.persist(&run);
        self.live(&run.id)?;
        Ok(run)
    }

    /// Start the grilling, or record answers to the pending questions and ask the next round.
    pub async fn grill(&self, id: &str, answers: Vec<String>) -> Result<Run, Fail> {
        let live = self.live(id)?;
        if live.grilling.swap(true, Ordering::SeqCst) {
            return Err(Fail::Conflict("the orchestrator is already thinking".into()));
        }
        let result = self.grill_inner(&live, answers).await;
        live.grilling.store(false, Ordering::SeqCst);
        result
    }

    async fn grill_inner(&self, live: &Live, answers: Vec<String>) -> Result<Run, Fail> {
        let (goal, project, flow, mut history, round, pages) = {
            let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
            if run.status != RunStatus::Grilling {
                return Err(Fail::Conflict("this run is past the questions".into()));
            }
            let (goal, project, flow) = (run.goal.clone(), run.project.clone(), run.flow.clone());
            let g = run.grill.get_or_insert_with(Grill::default);
            // Answers go to the pending questions in order; a blank answer takes the recommended one.
            let pending = std::mem::take(&mut g.pending);
            for (i, q) in pending.iter().enumerate() {
                let a = answers.get(i).map(|a| a.trim()).filter(|a| !a.is_empty()).unwrap_or(&q.recommended);
                g.history.push(Qa { question: q.text.clone(), answer: a.to_string() });
            }
            g.brief = None;
            let (history, round) = (g.history.clone(), g.round + 1);
            (goal, project, flow, history, round, run.references.clone())
        };
        let accounts = crate::secrets::load(&self.data_dir).await;
        let research = accounts.firecrawl().and_then(|a| crate::firecrawl::client::Client::new(a).ok());
        let (step, outcome) = prompts::grill(&goal, &project, &flow, &history, round, accounts.helper_endpoint("grill").as_ref(), &pages, research, &self.scratch).await?;
        let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
        run.overhead.add(&outcome.tokens);
        run.overhead_usd += outcome.cost_usd;
        run.research_credits += outcome.credits;
        let g = run.grill.get_or_insert_with(Grill::default);
        g.round = round;
        history.clear();
        match step {
            GrillStep::Questions(q) => g.pending = q,
            GrillStep::Brief(b) => g.brief = Some(b),
        }
        self.persist(&run);
        Ok(run.clone())
    }

    /// The user has confirmed the brief: run the flow.
    pub fn start(&self, st: &Arc<AppState>, id: &str, brief: Option<String>) -> Result<Run, Fail> {
        let live = self.live(id)?;
        {
            let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
            if run.status != RunStatus::Grilling {
                return Err(Fail::Conflict("this run has already started".into()));
            }
            let brief = brief
                .map(|b| b.trim().to_string())
                .filter(|b| !b.is_empty())
                .or_else(|| run.grill.as_ref().and_then(|g| g.brief.clone()))
                .ok_or_else(|| Fail::Bad("there is no brief to start from yet".into()))?;
            run.brief = brief;
            run.status = RunStatus::Running;
            self.persist(&run);
        }
        self.spawn_drive(st.clone(), live.clone());
        self.get(id)
    }

    pub fn approve(&self, st: &Arc<AppState>, id: &str, node: &str) -> Result<Run, Fail> {
        let live = self.live(id)?;
        {
            let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
            if run.flow.node(node).is_none() {
                return Err(Fail::Bad("no such agent in this flow".into()));
            }
            run.approved.insert(node.to_string());
            self.persist(&run);
        }
        let waiting = matches!(
            self.get(id)?.status,
            RunStatus::Paused { pause: Pause::Approval { node: ref n } } if n == node
        );
        if waiting {
            self.spawn_drive(st.clone(), live);
        }
        self.get(id)
    }

    pub fn limits(&self) -> super::graph::Budget {
        *self.limits.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_limits(&self, b: super::graph::Budget) -> Result<super::graph::Budget, Fail> {
        b.validate().map_err(Fail::Bad)?;
        self.store.save_limits(&b).map_err(Fail::Internal)?;
        *self.limits.lock().unwrap_or_else(|e| e.into_inner()) = b;
        Ok(b)
    }

    /// The user answers the questions the run is waiting on. The work goes back with the answers, and the run goes on.
    pub fn answer(&self, st: &Arc<AppState>, id: &str, answers: Vec<String>) -> Result<Run, Fail> {
        let live = self.live(id)?;
        {
            let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(run.status, RunStatus::Paused { pause: Pause::Questions { .. } }) {
                return Err(Fail::Conflict("this run is not waiting for answers".into()));
            }
            super::exec::answer_questions(&mut run, answers).map_err(Fail::Bad)?;
            self.persist(&run);
        }
        self.spawn_drive(st.clone(), live);
        self.get(id)
    }

    /// Carry on after a pause: a failure, a limit, or a restart. `waive` also stops the plan limits being checked
    /// again for this run, which is what "carry on anyway" means after a limit stopped it.
    pub fn resume(&self, st: &Arc<AppState>, id: &str, waive: bool) -> Result<Run, Fail> {
        let live = self.live(id)?;
        {
            let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
            match &run.status {
                RunStatus::Paused { pause: Pause::Questions { .. } } => return Err(Fail::Conflict("answer the questions first".into())),
                RunStatus::Paused { .. } => {}
                _ => return Err(Fail::Conflict("this run is not paused".into())),
            }
            // Nodes that were interrupted by a restart or failed are tried again.
            for n in run.nodes.values_mut().filter(|n| n.status == NodeStatus::Interrupted) {
                n.status = NodeStatus::Pending;
            }
            if waive && !run.waived {
                run.waived = true;
                live.runner.waive();
                run.note(None, super::run::LogKind::Decision, "You chose to carry on past the plan limits: they are not checked again for this run", None);
            }
            run.status = RunStatus::Running;
            self.persist(&run);
        }
        self.spawn_drive(st.clone(), live);
        self.get(id)
    }

    /// A run that stopped for having sent work back as often as allowed: allow more, and carry on.
    pub fn more_rounds(&self, st: &Arc<AppState>, id: &str, extra: u8) -> Result<Run, Fail> {
        let live = self.live(id)?;
        let running = {
            let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
            super::exec::more_rounds(&mut run, extra).map_err(Fail::Conflict)?;
            self.persist(&run);
            run.status == RunStatus::Running
        };
        if running {
            self.spawn_drive(st.clone(), live);
        }
        self.get(id)
    }

    pub fn cancel(&self, id: &str) -> Result<Run, Fail> {
        let live = self.live(id)?;
        live.cancelled.store(true, Ordering::SeqCst);
        if live.driving.load(Ordering::SeqCst) {
            live.cancel.notify_one();
        } else {
            let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
            if run.status != RunStatus::Done {
                run.status = RunStatus::Cancelled;
                run.finished = Some(super::run::now_secs());
                self.persist(&run);
            }
        }
        self.get(id)
    }

    /// Forget a run. One that is working is refused: stop it first. Branches and folders its builders made are kept.
    pub fn delete_run(&self, id: &str) -> Result<(), Fail> {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = runs.get(id) {
            if l.driving.load(Ordering::SeqCst) || l.grilling.load(Ordering::SeqCst) {
                return Err(Fail::Conflict("it is still working: stop it first".into()));
            }
        }
        self.store.delete_run(id).map_err(|_| Fail::Bad("bad id".into()))?;
        runs.remove(id);
        self.leases.revoke_prefix(&format!("orch:{id}:"));
        Ok(())
    }

    fn spawn_drive(&self, st: Arc<AppState>, live: Arc<Live>) {
        if live.driving.swap(true, Ordering::SeqCst) {
            return;
        }
        live.cancelled.store(false, Ordering::SeqCst);
        tokio::spawn(async move {
            let svc = &st.orch;
            svc.refresh_windows(&st).await;
            let refresher = {
                let st = st.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(WINDOW_REFRESH).await;
                        st.orch.refresh_windows(&st).await;
                    }
                })
            };
            let (windows, limits) = (svc.windows.clone(), svc.limits.clone());
            let limit = move |r: &Run| {
                let b = *limits.lock().unwrap_or_else(|e| e.into_inner());
                gate::check(r, &windows.lock().unwrap_or_else(|e| e.into_inner()), &b)
            };
            let (windows, limits) = (svc.windows.clone(), svc.limits.clone());
            let pressure = move |r: &Run, e: super::graph::Engine| {
                let b = *limits.lock().unwrap_or_else(|e| e.into_inner());
                gate::pressure(r, &windows.lock().unwrap_or_else(|x| x.into_inner()), &b, e)
            };
            let changed = |r: &Run| svc.persist(r);
            let record = |id: &str, d: &super::run::TurnDetail| {
                if let Err(e) = svc.store.append_turn(id, d) {
                    tracing::warn!("orchestrator: keeping turn {} of run {id}: {e:#}", d.n);
                }
            };
            let ctl = Control { cancelled: &live.cancelled, changed: &changed, limit: &limit, pressure: &pressure, record: &record };
            tokio::select! {
                _ = exec::drive(&live.shared, &live.runner, &ctl) => {}
                _ = live.cancel.notified() => {
                    // Dropping the drive future above killed any Claude agent mid-turn.
                    live.runner.interrupt().await;
                    let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
                    for n in run.nodes.values_mut().filter(|n| n.status == NodeStatus::Running) {
                        n.status = NodeStatus::Interrupted;
                    }
                    run.status = RunStatus::Cancelled;
                    run.finished = Some(super::run::now_secs());
                    svc.persist(&run);
                }
            }
            refresher.abort();
            // A run that is over needs no credentials any more; one that is paused keeps them to carry on.
            if matches!(live.shared.lock().unwrap_or_else(|e| e.into_inner()).status, RunStatus::Done | RunStatus::Cancelled) {
                live.runner.end_leases();
            }
            live.driving.store(false, Ordering::SeqCst);
        });
    }

    /// Flows the user can start from: their own, then the built-in ones.
    pub fn flows(&self) -> serde_json::Value {
        json!({"saved": self.store.list_flows(), "starters": super::starters::all()})
    }

    /// Open a builder's conversation in a live ttm session, in its worktree, so a person can carry on with
    /// it directly. Its headless agent is let go first: two processes must not write to one conversation.
    pub async fn pop_out(&self, st: &Arc<AppState>, run_id: &str, node_id: &str) -> Result<String, Fail> {
        let live = self.live(run_id)?;
        let (node, session, working) = {
            let run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
            let node = run.flow.node(node_id).cloned().ok_or(Fail::NotFound)?;
            let nr = run.nodes.get(node_id).ok_or(Fail::NotFound)?;
            (node, nr.session.clone(), nr.status == NodeStatus::Running)
        };
        if !node.writes() {
            return Err(Fail::Bad("only an agent that changes files can be opened in a live session".into()));
        }
        if working {
            return Err(Fail::Conflict("it is working right now: stop the run first, or wait for it to finish".into()));
        }
        let session = session.ok_or_else(|| Fail::Conflict("it has not finished a turn yet, so there is no conversation to continue".into()))?;
        let dir = live.runner.worktree_dir(node_id);
        if !dir.is_dir() {
            return Err(Fail::Conflict("its worktree is gone".into()));
        }
        // Codex's headless server keeps a thread locked against being opened elsewhere, so a Codex builder is forked: the person
        // gets a copy of its conversation to carry on with. Claude's conversation is resumed as it is.
        let resume_id = if node.engine == super::graph::Engine::Codex { format!("{}{session}", crate::sessions::FORK) } else { session.clone() };
        // An agent that ran on OpenRouter carries on there, on the same model, not on your own login.
        if let Some(account) = node.account.as_deref() {
            live.runner.release(node_id).await;
            let name = format!("orch-{node_id}-{}", &run_id[..run_id.len().min(10)]);
            let driver = match node.engine {
                super::graph::Engine::Claude => crate::openrouter::launch::Driver::Claude,
                super::graph::Engine::Codex => crate::openrouter::launch::Driver::Codex,
            };
            let req = crate::sessions::OrNew { account, model: &node.model, driver: Some(driver), cap_usd: None, name: Some(&name), dir: Some(dir), yolo: false, resume: Some(&resume_id) };
            return crate::sessions::create_openrouter(st, req).await.map_err(Fail::Internal);
        }
        let wanted = match node.engine {
            super::graph::Engine::Claude => crate::config::Agent::Claude,
            super::graph::Engine::Codex => crate::config::Agent::Codex,
        };
        let preset = st
            .cfg
            .presets
            .iter()
            .find(|p| p.agent() == wanted)
            .ok_or_else(|| Fail::Bad(format!("no preset in the config starts {}", if wanted == crate::config::Agent::Claude { "Claude Code" } else { "Codex" })))?;
        live.runner.release(node_id).await;
        let name = format!("orch-{node_id}-{}", &run_id[..run_id.len().min(10)]);
        let req = crate::sessions::NewSession { preset: Some(&preset.name), name: Some(&name), group: None, exact: false, yolo: false, dir: Some(dir) };
        crate::sessions::create_resuming(st, req, Some(&resume_id)).await.map_err(Fail::Internal)
    }

    pub fn save_flow(&self, flow: &Flow) -> Result<(), Fail> {
        flow.validate().map_err(Fail::Bad)?;
        self.store.save_flow(flow).context("saving the flow")?;
        Ok(())
    }
}
