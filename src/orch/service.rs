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
    scratch: PathBuf,
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
    pub fn new(data_dir: &Path) -> Service {
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
            store,
            codex: Arc::default(),
            runs: Mutex::default(),
            events: broadcast::channel(256).0,
            windows: Arc::default(),
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
        let live = Arc::new(Live {
            runner: EngineRunner::new(
                run.flow.clone(),
                PathBuf::from(&run.project),
                self.store.worktrees(),
                run.id.clone(),
                self.codex.clone(),
            ),
            shared: Mutex::new(run),
            cancelled: AtomicBool::new(false),
            cancel: Notify::new(),
            driving: AtomicBool::new(false),
            grilling: AtomicBool::new(false),
        });
        runs.insert(id.to_string(), live.clone());
        Ok(live)
    }

    pub fn get(&self, id: &str) -> Result<Run, Fail> {
        Ok(self.live(id)?.shared.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// Models each engine offers. Codex's are asked of Codex itself, so they never go stale.
    pub async fn models(&self) -> serde_json::Value {
        let codex = match self.codex.get().await {
            Ok(s) => s.models().await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let codex = if codex.is_empty() { FALLBACK_CODEX.iter().map(|s| s.to_string()).collect() } else { codex };
        json!({"claude": prompts::CLAUDE_MODELS, "codex": codex})
    }

    async fn codex_models(&self) -> Vec<String> {
        let m = self.models().await;
        serde_json::from_value(m["codex"].clone()).unwrap_or_default()
    }

    pub async fn suggest_flow(&self, st: &Arc<AppState>, goal: &str) -> Result<Flow, Fail> {
        let _ = st;
        if goal.trim().is_empty() {
            return Err(Fail::Bad("say what you want done".into()));
        }
        let models = self.codex_models().await;
        let id = format!("f{}", new_id());
        let (flow, _) = prompts::suggest_flow(goal, &id, &models, &self.scratch).await?;
        Ok(flow)
    }

    pub async fn node_prompt(&self, role: super::graph::Role, name: &str, goal: &str) -> Result<String, Fail> {
        Ok(prompts::node_prompt(role, name, goal, &self.scratch).await?.0)
    }

    pub async fn create_run(
        &self,
        st: &Arc<AppState>,
        flow: Flow,
        goal: String,
        project: &str,
        skip_grill: bool,
    ) -> Result<Run, Fail> {
        flow.validate().map_err(Fail::Bad)?;
        if goal.trim().is_empty() {
            return Err(Fail::Bad("say what you want done".into()));
        }
        let project = std::fs::canonicalize(project).map_err(|_| Fail::Bad("that project folder does not exist".into()))?;
        if !project.is_dir() {
            return Err(Fail::Bad("the project must be a folder".into()));
        }
        let w = self.refresh_windows(st).await;
        let mut run = Run::new(new_id(), flow, goal.trim().to_string(), now());
        run.project = project.to_string_lossy().into_owned();
        run.baseline = w.baseline();
        run.status = RunStatus::Grilling;
        run.grill = Some(Grill {
            brief: skip_grill.then(|| goal.trim().to_string()),
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
        let (goal, project, flow, mut history, round) = {
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
            (goal, project, flow, g.history.clone(), g.round + 1)
        };
        let (step, outcome) = prompts::grill(&goal, &project, &flow, &history, round, &self.scratch).await?;
        let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
        run.overhead.add(&outcome.tokens);
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

    /// Carry on after a pause: a failure, a limit, or a restart.
    pub fn resume(&self, st: &Arc<AppState>, id: &str) -> Result<Run, Fail> {
        let live = self.live(id)?;
        {
            let mut run = live.shared.lock().unwrap_or_else(|e| e.into_inner());
            match &run.status {
                RunStatus::Paused { .. } => {}
                _ => return Err(Fail::Conflict("this run is not paused".into())),
            }
            // Nodes that were interrupted by a restart or failed are tried again.
            for n in run.nodes.values_mut().filter(|n| n.status == NodeStatus::Interrupted) {
                n.status = NodeStatus::Pending;
            }
            run.status = RunStatus::Running;
            self.persist(&run);
        }
        self.spawn_drive(st.clone(), live);
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
                self.persist(&run);
            }
        }
        self.get(id)
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
            let windows = svc.windows.clone();
            let limit = move |r: &Run| gate::check(r, &windows.lock().unwrap_or_else(|e| e.into_inner()));
            let changed = |r: &Run| svc.persist(r);
            let ctl = Control { cancelled: &live.cancelled, changed: &changed, limit: &limit };
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
                    svc.persist(&run);
                }
            }
            refresher.abort();
            live.driving.store(false, Ordering::SeqCst);
        });
    }

    /// Flows the user can start from: their own, then the built-in ones.
    pub fn flows(&self) -> serde_json::Value {
        json!({"saved": self.store.list_flows(), "starters": super::starters::all()})
    }

    pub fn save_flow(&self, flow: &Flow) -> Result<(), Fail> {
        flow.validate().map_err(Fail::Bad)?;
        self.store.save_flow(flow).context("saving the flow")?;
        Ok(())
    }
}
