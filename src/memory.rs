//! A project's memsearch memory: the daily notes under `.memsearch/memory`, a semantic search
//! over them, a topic rollup written by memsearch's own LLM, and re-indexing. memsearch is a
//! Python CLI, so searching and indexing shell out to it; reading the notes does not.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{process::Command, sync::Semaphore};

use crate::AppState;

const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);
const STATS_TIMEOUT: Duration = Duration::from_secs(15);
const INDEX_TIMEOUT: Duration = Duration::from_secs(600);
const LLM_TIMEOUT: Duration = Duration::from_secs(120);
const CONFIG_TTL: Duration = Duration::from_secs(60);
const MAX_QUERY: usize = 500;
const MAX_RESULTS: usize = 20;
/// Searches kept so that paging back to one is instant.
const SEARCH_CACHE: usize = 32;
/// How much of the notes the rollup sends to the LLM, newest first.
const ROLLUP_CHARS: usize = 24_000;
const ROLLUP_DAYS: usize = 20;

/// Where a project's memory lives and what memsearch calls its index.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub root: PathBuf,
    pub memory_dir: PathBuf,
    pub collection: String,
}

/// The project that owns `cwd`: the nearest folder with a `.memsearch` (the home folder only
/// counts when `cwd` is the home folder, since its `.memsearch` holds the global memory), else
/// the git root, else `cwd` itself.
pub fn resolve(cwd: &Path, home: Option<&Path>) -> Project {
    let root = cwd
        .ancestors()
        .find(|d| d.join(".memsearch").is_dir() && (Some(*d) != home || cwd == *d))
        .or_else(|| cwd.ancestors().find(|d| d.join(".git").exists()))
        .unwrap_or(cwd)
        .to_path_buf();
    let collection = index_state(&root)
        .and_then(|s| s.collection)
        .filter(|c| valid_collection(c))
        .unwrap_or_else(|| derive_collection(&root));
    Project { memory_dir: root.join(".memsearch").join("memory"), root, collection }
}

/// `ms_<basename>_<8 hex of sha256(path)>`, the same rule as the memsearch plugin's
/// `derive-collection.sh`, so the tab reads the index the agent writes.
pub fn derive_collection(root: &Path) -> String {
    let abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let name = abs.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut slug = String::new();
    for c in name.chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '_' };
        if c != '_' || !slug.ends_with('_') {
            slug.push(c);
        }
    }
    let slug: String = slug.trim_matches('_').chars().take(40).collect();
    let hash: String = Sha256::digest(abs.to_string_lossy().as_bytes())[..4].iter().map(|b| format!("{b:02x}")).collect();
    format!("ms_{slug}_{hash}")
}

/// Collection names become file names and command arguments, so only plain ones are used.
fn valid_collection(c: &str) -> bool {
    !c.is_empty() && c.len() <= 120 && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

#[derive(Deserialize, Default)]
struct IndexState {
    collection: Option<String>,
    status: Option<String>,
    last_success_at: Option<String>,
}

fn index_state_path(root: &Path) -> PathBuf {
    root.join(".memsearch").join(".index-state.json")
}

fn index_state(root: &Path) -> Option<IndexState> {
    serde_json::from_slice(&std::fs::read(index_state_path(root)).ok()?).ok()
}

fn mtime_secs(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `YYYY-MM-DD`, the name of a daily note.
pub fn valid_date(d: &str) -> bool {
    d.len() == 10
        && d.char_indices().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() })
}

/// The daily notes, newest first, as `(date, path)`.
fn day_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let date = path.file_name()?.to_str()?.strip_suffix(".md")?.to_owned();
            valid_date(&date).then_some((date, path))
        })
        .collect();
    v.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    v
}

/// Changes whenever a note is added or touched; the rollup is stale once it differs.
fn stamp(dir: &Path) -> String {
    let files = day_files(dir);
    let newest = files.iter().filter_map(|(_, p)| mtime_secs(p)).max().unwrap_or(0);
    format!("{}:{newest}", files.len())
}

// ---------------------------------------------------------------- the notes

#[derive(Debug, Serialize, PartialEq)]
pub struct Entry {
    pub time: String,
    /// The `## Session 12:11` heading the entry sits under, if the note has them.
    pub group: Option<String>,
    /// The agent conversation the entry was written from.
    pub session: Option<String>,
    pub bullets: Vec<String>,
}

/// `<!-- session:ID turn:ID transcript:PATH -->`: the value after `key`.
fn comment_field(line: &str, key: &str) -> Option<String> {
    let rest = line.split_once(key)?.1;
    let v = rest.split_whitespace().next()?.trim_end_matches("-->");
    (!v.is_empty()).then(|| v.to_owned())
}

pub fn parse_day(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut group: Option<String> = None;
    let mut cur: Option<Entry> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(time) = line.strip_prefix("### ") {
            out.extend(cur.take());
            cur = Some(Entry { time: time.trim().to_owned(), group: group.clone(), session: None, bullets: Vec::new() });
        } else if let Some(g) = line.strip_prefix("## ") {
            out.extend(cur.take());
            group = Some(g.trim().to_owned());
        } else if line.starts_with("# ") || line.trim().is_empty() {
            continue;
        } else if line.starts_with("<!--") {
            if let Some(e) = cur.as_mut() {
                e.session = e.session.take().or_else(|| comment_field(line, "session:"));
            }
        } else if let Some(e) = cur.as_mut() {
            let text = line.trim();
            if let Some(bullet) = text.strip_prefix("- ") {
                e.bullets.push(bullet.trim().to_owned());
            } else if let Some(last) = e.bullets.last_mut() {
                last.push(' ');
                last.push_str(text);
            } else {
                e.bullets.push(text.to_owned());
            }
        }
    }
    out.extend(cur);
    out
}

/// The note text without the `<!-- ... -->` anchors, and the session the anchor names.
fn strip_anchors(content: &str) -> (String, Option<String>) {
    let mut session = None;
    let kept: Vec<&str> = content
        .lines()
        .filter(|l| {
            if l.trim_start().starts_with("<!--") {
                session = session.take().or_else(|| comment_field(l, "session:"));
                false
            } else {
                true
            }
        })
        .collect();
    (kept.join("\n").trim().to_owned(), session)
}

// ---------------------------------------------------------------- memsearch

/// Where `memsearch` is installed: the service's PATH is often shorter than a shell's.
fn memsearch_bin() -> PathBuf {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(h) = crate::agentlog::home() {
        dirs.push(h.join(".local/bin"));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    dirs.into_iter().map(|d| d.join("memsearch")).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("memsearch"))
}

fn last_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output").trim().to_owned()
}

async fn run(args: &[&str], dir: &Path, limit: Duration) -> Result<Vec<u8>> {
    let child = Command::new(memsearch_bin())
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("starting memsearch (is it installed?)")?;
    let out = tokio::time::timeout(limit, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("memsearch took longer than {}s", limit.as_secs()))?
        .context("running memsearch")?;
    if !out.status.success() {
        bail!("memsearch failed: {}", last_line(&out.stderr));
    }
    Ok(out.stdout)
}

/// What `memsearch config list` resolves to for a project.
#[derive(Clone, Default)]
struct MemConfig {
    provider: Option<String>,
    model: Option<String>,
    llm_model: Option<String>,
    llm_base: Option<String>,
    llm_key: Option<String>,
}

fn parse_config(text: &str) -> MemConfig {
    let Ok(t) = text.parse::<toml::Table>() else { return MemConfig::default() };
    let get = |table: &str, key: &str| {
        t.get(table).and_then(|v| v.get(key)).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_owned)
    };
    MemConfig {
        provider: get("embedding", "provider"),
        model: get("embedding", "model"),
        llm_model: get("compact", "llm_model"),
        llm_base: get("compact", "base_url"),
        llm_key: get("compact", "api_key"),
    }
}

// ---------------------------------------------------------------- search

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Hit {
    pub chunk_hash: String,
    pub date: String,
    pub heading: String,
    pub score: f64,
    pub text: String,
    pub session: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Deserialize)]
struct RawHit {
    chunk_hash: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    heading: Option<String>,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    end_line: Option<u32>,
}

impl From<RawHit> for Hit {
    fn from(r: RawHit) -> Self {
        let (text, session) = strip_anchors(&r.content);
        let date = Path::new(&r.source).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Hit {
            chunk_hash: r.chunk_hash,
            date,
            heading: r.heading.unwrap_or_default(),
            score: r.score.unwrap_or(0.0),
            text,
            session,
            start_line: r.start_line.unwrap_or(0),
            end_line: r.end_line.unwrap_or(0),
        }
    }
}

/// memsearch prints JSON on stdout; anything before the first `[` or `{` is noise.
fn json_start(out: &[u8], open: u8) -> &[u8] {
    out.iter().position(|&b| b == open).map_or(out, |i| &out[i..])
}

type SearchKey = (String, String, usize, u64);

#[derive(Clone, Default, Serialize)]
pub struct Job {
    pub running: bool,
    pub started: u64,
    pub finished: u64,
    pub error: Option<String>,
}

pub struct Memory {
    http: reqwest::Client,
    /// memsearch loads a model per run, so only a couple at a time.
    gate: Semaphore,
    searches: Mutex<VecDeque<(SearchKey, Vec<Hit>)>>,
    configs: Mutex<HashMap<PathBuf, (Instant, MemConfig)>>,
    chunks: Mutex<HashMap<String, (u64, u64)>>,
    reindex: Mutex<HashMap<String, Job>>,
    rollup: Mutex<HashMap<String, Job>>,
}

impl Memory {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(LLM_TIMEOUT)
                .user_agent(concat!("ttm/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            gate: Semaphore::new(2),
            searches: Mutex::default(),
            configs: Mutex::default(),
            chunks: Mutex::default(),
            reindex: Mutex::default(),
            rollup: Mutex::default(),
        }
    }

    async fn config(&self, p: &Project) -> MemConfig {
        if let Some((_, c)) = lock(&self.configs).get(&p.root).filter(|(at, _)| at.elapsed() < CONFIG_TTL) {
            return c.clone();
        }
        let cfg = match run(&["config", "list"], &p.root, STATS_TIMEOUT).await {
            Ok(out) => parse_config(&String::from_utf8_lossy(&out)),
            Err(e) => {
                tracing::debug!("memsearch config: {e:#}");
                MemConfig::default()
            }
        };
        lock(&self.configs).insert(p.root.clone(), (Instant::now(), cfg.clone()));
        cfg
    }

    /// Chunks in the index, re-counted only when the index changed.
    async fn chunk_count(&self, p: &Project) -> Option<u64> {
        let version = mtime_secs(&index_state_path(&p.root)).unwrap_or(0);
        if let Some(&(v, n)) = lock(&self.chunks).get(&p.collection)
            && v == version
        {
            return Some(n);
        }
        let _permit = self.gate.acquire().await.ok()?;
        let out = run(&["stats", "-c", &p.collection], &p.root, STATS_TIMEOUT).await.ok()?;
        let n = String::from_utf8_lossy(&out)
            .lines()
            .find_map(|l| l.split_once("Total indexed chunks:")?.1.trim().parse::<u64>().ok())?;
        lock(&self.chunks).insert(p.collection.clone(), (version, n));
        Some(n)
    }
}

pub async fn search(st: &AppState, p: &Project, query: &str, k: usize) -> Result<Vec<Hit>> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > MAX_QUERY {
        bail!("the query must be 1 to {MAX_QUERY} characters");
    }
    let k = k.clamp(1, MAX_RESULTS);
    let key: SearchKey = (p.collection.clone(), query.to_owned(), k, mtime_secs(&index_state_path(&p.root)).unwrap_or(0));
    {
        let mut cache = lock(&st.memory.searches);
        if let Some(i) = cache.iter().position(|(kk, _)| *kk == key) {
            let hit = cache.remove(i).expect("position is in range");
            let hits = hit.1.clone();
            cache.push_front(hit);
            return Ok(hits);
        }
    }
    let _permit = st.memory.gate.acquire().await.context("memory search is shutting down")?;
    let kk = k.to_string();
    let out = run(&["search", "-j", "-k", &kk, "-c", &p.collection, "--", query], &p.root, SEARCH_TIMEOUT).await?;
    let raw: Vec<RawHit> = serde_json::from_slice(json_start(&out, b'[')).context("reading memsearch's results")?;
    let hits: Vec<Hit> = raw.into_iter().map(Hit::from).collect();
    let mut cache = lock(&st.memory.searches);
    cache.push_front((key, hits.clone()));
    cache.truncate(SEARCH_CACHE);
    Ok(hits)
}

#[derive(Serialize)]
pub struct Expanded {
    pub chunk_hash: String,
    pub date: String,
    pub heading: String,
    pub text: String,
    pub session: Option<String>,
}

pub async fn expand(st: &AppState, p: &Project, hash: &str) -> Result<Expanded> {
    if hash.len() < 8 || hash.len() > 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("not a chunk hash");
    }
    let _permit = st.memory.gate.acquire().await.context("memory search is shutting down")?;
    let out = run(&["expand", hash, "-j", "-c", &p.collection], &p.root, SEARCH_TIMEOUT).await?;
    let raw: RawHit = serde_json::from_slice(json_start(&out, b'{')).context("reading memsearch's answer")?;
    let hit = Hit::from(raw);
    Ok(Expanded { chunk_hash: hit.chunk_hash, date: hit.date, heading: hit.heading, text: hit.text, session: hit.session })
}

// ---------------------------------------------------------------- overview

#[derive(Serialize)]
pub struct DaySummary {
    pub date: String,
    pub entries: usize,
    pub bytes: u64,
}

#[derive(Serialize)]
pub struct Overview {
    pub root: String,
    pub collection: String,
    pub days: Vec<DaySummary>,
    pub entries: usize,
    pub chunks: Option<u64>,
    pub index_status: Option<String>,
    pub last_indexed: Option<String>,
    /// A note is newer than the last successful index run.
    pub unindexed: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub llm_model: Option<String>,
    pub reindex: Job,
    pub rollup: Job,
}

fn summarize_days(dir: &Path) -> Vec<DaySummary> {
    day_files(dir)
        .into_iter()
        .map(|(date, path)| {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let entries = text.lines().filter(|l| l.starts_with("### ")).count();
            DaySummary { date, entries, bytes: text.len() as u64 }
        })
        .collect()
}

pub async fn overview(st: &AppState, p: &Project) -> Overview {
    let dir = p.memory_dir.clone();
    let days = tokio::task::spawn_blocking(move || summarize_days(&dir)).await.unwrap_or_default();
    let (cfg, chunks) = tokio::join!(st.memory.config(p), st.memory.chunk_count(p));
    let state = index_state(&p.root);
    let indexed_at = state
        .as_ref()
        .and_then(|s| s.last_success_at.as_deref())
        .and_then(|t| time::OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339).ok())
        .map(|t| t.unix_timestamp().max(0) as u64);
    let newest = day_files(&p.memory_dir).iter().filter_map(|(_, f)| mtime_secs(f)).max();
    let unindexed = match (newest, indexed_at) {
        (Some(n), Some(i)) => n > i + 1,
        (Some(_), None) => true,
        _ => false,
    };
    Overview {
        root: p.root.display().to_string(),
        collection: p.collection.clone(),
        entries: days.iter().map(|d| d.entries).sum(),
        days,
        chunks,
        index_status: state.as_ref().and_then(|s| s.status.clone()),
        last_indexed: state.and_then(|s| s.last_success_at),
        unindexed,
        provider: cfg.provider,
        model: cfg.model,
        llm_model: cfg.llm_model,
        reindex: job(&st.memory.reindex, &p.collection),
        rollup: job(&st.memory.rollup, &p.collection),
    }
}

pub async fn day(p: &Project, date: &str) -> Option<Vec<Entry>> {
    if !valid_date(date) {
        return None;
    }
    let path = p.memory_dir.join(format!("{date}.md"));
    let text = tokio::fs::read_to_string(path).await.ok()?;
    Some(parse_day(&text))
}

// ---------------------------------------------------------------- jobs

fn job(jobs: &Mutex<HashMap<String, Job>>, collection: &str) -> Job {
    lock(jobs).get(collection).cloned().unwrap_or_default()
}

/// Mark `collection`'s job running, unless one already is.
fn begin(jobs: &Mutex<HashMap<String, Job>>, collection: &str) -> Result<Job, Job> {
    let mut jobs = lock(jobs);
    let j = jobs.entry(collection.to_owned()).or_default();
    if j.running {
        return Err(j.clone());
    }
    *j = Job { running: true, started: now_secs(), ..Job::default() };
    Ok(j.clone())
}

fn finish(jobs: &Mutex<HashMap<String, Job>>, collection: &str, result: Result<()>) {
    let mut jobs = lock(jobs);
    let j = jobs.entry(collection.to_owned()).or_default();
    j.running = false;
    j.finished = now_secs();
    j.error = result.err().map(|e| format!("{e:#}"));
}

/// Index the project's notes in the background. Returns the job as it stands.
pub fn start_reindex(st: &Arc<AppState>, p: Project) -> Job {
    let started = match begin(&st.memory.reindex, &p.collection) {
        Ok(j) => j,
        Err(running) => return running,
    };
    let st = st.clone();
    tokio::spawn(async move {
        let dir = p.memory_dir.display().to_string();
        let result = async {
            let _permit = st.memory.gate.acquire().await.context("memory search is shutting down")?;
            run(&["index", &dir, "-c", &p.collection], &p.root, INDEX_TIMEOUT).await.map(|_| ())
        }
        .await;
        finish(&st.memory.reindex, &p.collection, result);
    });
    started
}

// ---------------------------------------------------------------- rollup

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Item {
    pub text: String,
    pub date: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Topic {
    pub name: String,
    pub summary: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct RollupBody {
    pub decisions: Vec<Item>,
    pub open_questions: Vec<Item>,
    pub topics: Vec<Topic>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LooseItem {
    Text(String),
    Full {
        #[serde(default)]
        text: String,
        #[serde(default)]
        date: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LooseTopic {
    Text(String),
    Full {
        #[serde(default)]
        name: String,
        #[serde(default)]
        summary: String,
    },
}

#[derive(Deserialize, Default)]
struct LooseBody {
    #[serde(default)]
    decisions: Vec<LooseItem>,
    #[serde(default)]
    open_questions: Vec<LooseItem>,
    #[serde(default)]
    topics: Vec<LooseTopic>,
}

impl From<LooseBody> for RollupBody {
    fn from(b: LooseBody) -> Self {
        let item = |i: LooseItem| match i {
            LooseItem::Text(text) => Item { text, date: None },
            LooseItem::Full { text, date } => Item { text, date },
        };
        RollupBody {
            decisions: b.decisions.into_iter().map(item).collect(),
            open_questions: b.open_questions.into_iter().map(item).collect(),
            topics: b
                .topics
                .into_iter()
                .map(|t| match t {
                    LooseTopic::Text(name) => Topic { name, summary: String::new() },
                    LooseTopic::Full { name, summary } => Topic { name, summary },
                })
                .collect(),
        }
    }
}

/// The JSON object in an LLM answer, which may come wrapped in prose or a code fence.
fn parse_rollup(answer: &str) -> Result<RollupBody> {
    let (start, end) = (answer.find('{'), answer.rfind('}'));
    let json = match (start, end) {
        (Some(s), Some(e)) if e > s => &answer[s..=e],
        _ => bail!("the model did not answer with JSON"),
    };
    let loose: LooseBody = serde_json::from_str(json).context("reading the model's answer")?;
    Ok(loose.into())
}

#[derive(Serialize, Deserialize)]
struct Stored {
    generated_at: u64,
    stamp: String,
    model: String,
    body: RollupBody,
}

fn rollup_path(st: &AppState, collection: &str) -> PathBuf {
    st.data_dir.join("memory-rollups").join(format!("{collection}.json"))
}

#[derive(Serialize)]
pub struct RollupView {
    pub job: Job,
    pub rollup: Option<RollupInfo>,
}

#[derive(Serialize)]
pub struct RollupInfo {
    pub generated_at: u64,
    pub model: String,
    /// Notes were added or changed after the rollup was written.
    pub stale: bool,
    #[serde(flatten)]
    pub body: RollupBody,
}

pub async fn rollup(st: &AppState, p: &Project) -> RollupView {
    let stored: Option<Stored> = tokio::fs::read(rollup_path(st, &p.collection)).await.ok().and_then(|b| serde_json::from_slice(&b).ok());
    let current = stamp(&p.memory_dir);
    RollupView {
        job: job(&st.memory.rollup, &p.collection),
        rollup: stored.map(|s| RollupInfo { stale: s.stamp != current, generated_at: s.generated_at, model: s.model, body: s.body }),
    }
}

/// The newest notes, anchors removed, trimmed to what one prompt can hold.
fn notes_for_prompt(dir: &Path) -> String {
    let mut out = String::new();
    for (date, path) in day_files(dir).into_iter().take(ROLLUP_DAYS) {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let (clean, _) = strip_anchors(&text);
        out.push_str(&format!("## {date}\n{clean}\n\n"));
        if out.len() >= ROLLUP_CHARS {
            break;
        }
    }
    if out.len() > ROLLUP_CHARS {
        let mut cut = ROLLUP_CHARS;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

const ROLLUP_PROMPT: &str = "You summarise a developer's memory notes about one software project. \
The notes are dated sections of bullet points written after each working session. Answer with a single JSON object and nothing else, shaped as: \
{\"decisions\":[{\"text\":\"...\",\"date\":\"YYYY-MM-DD\"}],\"open_questions\":[{\"text\":\"...\",\"date\":\"YYYY-MM-DD\"}],\"topics\":[{\"name\":\"...\",\"summary\":\"...\"}]}. \
decisions are choices that were made and still stand. open_questions are things left unresolved, unverified, or asked of the user and not yet answered; leave out anything a later note shows was resolved. \
topics are the recurring areas of work, each with a one or two sentence summary. Use the date of the note the item comes from. Keep every item to one sentence, at most 12 of each kind, most recent first.";

async fn generate(st: &AppState, p: &Project) -> Result<()> {
    let cfg = st.memory.config(p).await;
    let (Some(base), Some(model)) = (cfg.llm_base.as_deref(), cfg.llm_model.as_deref()) else {
        bail!("memsearch has no summarizing LLM configured (see `memsearch config list`)");
    };
    let dir = p.memory_dir.clone();
    let before = stamp(&dir);
    let notes = tokio::task::spawn_blocking({
        let dir = dir.clone();
        move || notes_for_prompt(&dir)
    })
    .await
    .context("reading the notes")?;
    if notes.trim().is_empty() {
        bail!("there are no notes to summarise yet");
    }
    let mut req = st.memory.http.post(format!("{}/chat/completions", base.trim_end_matches('/'))).json(&serde_json::json!({
        "model": model,
        "temperature": 0.2,
        "messages": [
            {"role": "system", "content": ROLLUP_PROMPT},
            {"role": "user", "content": notes},
        ],
    }));
    if let Some(key) = cfg.llm_key.as_deref() {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await.context("calling the summarizing LLM")?;
    let status = resp.status();
    let value: serde_json::Value = resp.json().await.context("reading the LLM's reply")?;
    if !status.is_success() {
        let msg = value.pointer("/error/message").and_then(|m| m.as_str()).unwrap_or("no message");
        bail!("the LLM answered {status}: {msg}");
    }
    let answer = value.pointer("/choices/0/message/content").and_then(|c| c.as_str()).context("the LLM's reply had no text")?;
    let body = parse_rollup(answer)?;
    let stored = Stored { generated_at: now_secs(), stamp: before, model: model.to_owned(), body };
    let path = rollup_path(st, &p.collection);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.context("creating the rollup folder")?;
    }
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, serde_json::to_vec(&stored)?).await.context("saving the rollup")?;
    // A summary of private notes: owner-only, set before the rename makes it visible.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).await.context("saving the rollup")?;
    }
    tokio::fs::rename(&tmp, &path).await.context("saving the rollup")?;
    Ok(())
}

/// Write the rollup in the background. Returns the job as it stands.
pub fn start_rollup(st: &Arc<AppState>, p: Project) -> Job {
    let started = match begin(&st.memory.rollup, &p.collection) {
        Ok(j) => j,
        Err(running) => return running,
    };
    let st = st.clone();
    tokio::spawn(async move {
        let result = generate(&st, &p).await;
        if let Err(e) = &result {
            tracing::warn!("memory rollup for {}: {e:#}", p.collection);
        }
        finish(&st.memory.rollup, &p.collection, result);
    });
    started
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ttm-memory-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn collection_matches_the_plugin_script() {
        // The name memsearch itself gave this repository.
        assert_eq!(derive_collection(Path::new("/Users/sandeep/Documents/odev/ttmassive")), "ms_ttmassive_5226a436");
    }

    #[test]
    fn collection_slug_is_sanitised() {
        let name = derive_collection(Path::new("/nonexistent/My  App!!-v2"));
        assert!(name.starts_with("ms_my_app_v2_"), "{name}");
        assert_eq!(name.len(), "ms_my_app_v2_".len() + 8);
    }

    #[test]
    fn collection_names_are_checked() {
        assert!(valid_collection("ms_ttmassive_5226a436"));
        assert!(!valid_collection("../x"));
        assert!(!valid_collection(""));
        assert!(!valid_collection("a b"));
    }

    #[test]
    fn dates_are_checked() {
        assert!(valid_date("2026-10-14"));
        assert!(!valid_date("2026-10-1"));
        assert!(!valid_date("../../etc"));
        assert!(!valid_date("2026_10_14"));
    }

    const NOTE: &str = "## Session 12:11
### 12:20
<!-- session:abc-123 turn:t1 transcript:/x/y.jsonl -->
- User asked for a tunnel option.
- Claude added it,
  then tested it.
### 12:25
<!-- session:abc-123 turn:t2 transcript:/x/y.jsonl -->
- Latency work.

## Session 12:33
### 12:36
- No anchor here.
";

    #[test]
    fn notes_are_parsed_into_entries() {
        let e = parse_day(NOTE);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].time, "12:20");
        assert_eq!(e[0].group.as_deref(), Some("Session 12:11"));
        assert_eq!(e[0].session.as_deref(), Some("abc-123"));
        assert_eq!(e[0].bullets, ["User asked for a tunnel option.", "Claude added it, then tested it."]);
        assert_eq!(e[1].bullets, ["Latency work."]);
        assert_eq!(e[2].group.as_deref(), Some("Session 12:33"));
        assert_eq!(e[2].session, None);
    }

    #[test]
    fn a_note_without_groups_parses() {
        let e = parse_day("### 08:29\n<!-- session:s1 turn:t -->\n- one\n");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].group, None);
    }

    #[test]
    fn anchors_are_stripped_from_search_text() {
        let (text, session) = strip_anchors("### 23:43\n<!-- session:s9 turn:t transcript:/a/b -->\n- hello\n");
        assert_eq!(text, "### 23:43\n- hello");
        assert_eq!(session.as_deref(), Some("s9"));
    }

    #[test]
    fn a_search_result_becomes_a_hit() {
        let raw: Vec<RawHit> = serde_json::from_str(
            r####"[{"chunk_hash":"467f","content":"### 23:43\n<!-- session:s1 -->\n- x","source":"/p/.memsearch/memory/2026-10-09.md","heading":"23:43","score":0.87,"start_line":298,"end_line":309,"heading_level":3}]"####,
        )
        .unwrap();
        let hit = Hit::from(raw.into_iter().next().unwrap());
        assert_eq!(hit.date, "2026-10-09");
        assert_eq!(hit.text, "### 23:43\n- x");
        assert_eq!(hit.session.as_deref(), Some("s1"));
        assert_eq!((hit.start_line, hit.end_line), (298, 309));
    }

    #[test]
    fn json_noise_before_the_data_is_skipped() {
        assert_eq!(json_start(b"warning: no token\n[1]", b'['), b"[1]");
        assert_eq!(json_start(b"[1]", b'['), b"[1]");
    }

    #[test]
    fn project_prefers_the_nearest_memsearch_folder() {
        let base = tmp("resolve");
        let home = base.join("home");
        let proj = home.join("code/app");
        fs::create_dir_all(proj.join(".memsearch")).unwrap();
        fs::create_dir_all(proj.join("src/deep")).unwrap();
        fs::create_dir_all(home.join(".memsearch")).unwrap();
        let p = resolve(&proj.join("src/deep"), Some(&home));
        assert_eq!(p.root, proj);
        assert_eq!(p.memory_dir, proj.join(".memsearch/memory"));
        assert_eq!(p.collection, derive_collection(&proj));
    }

    #[test]
    fn the_home_memory_is_not_a_projects_memory() {
        let base = tmp("home");
        let home = base.join("home");
        let proj = home.join("code/other");
        fs::create_dir_all(home.join(".memsearch")).unwrap();
        fs::create_dir_all(proj.join(".git")).unwrap();
        fs::create_dir_all(proj.join("sub")).unwrap();
        assert_eq!(resolve(&proj.join("sub"), Some(&home)).root, proj, "falls back to the git root");
        assert_eq!(resolve(&home, Some(&home)).root, home, "but the home folder itself uses its own");
        let bare = home.join("scratch");
        fs::create_dir_all(&bare).unwrap();
        assert_eq!(resolve(&bare, Some(&home)).root, bare, "and with no git root, the folder itself");
    }

    #[test]
    fn the_index_state_names_the_collection() {
        let base = tmp("state");
        fs::create_dir_all(base.join(".memsearch")).unwrap();
        fs::write(index_state_path(&base), r#"{"collection":"ms_custom_1234abcd","status":"ok"}"#).unwrap();
        assert_eq!(resolve(&base, None).collection, "ms_custom_1234abcd");
        fs::write(index_state_path(&base), r#"{"collection":"../escape"}"#).unwrap();
        assert_eq!(resolve(&base, None).collection, derive_collection(&base), "an unsafe name is ignored");
    }

    #[test]
    fn the_stamp_changes_with_the_notes() {
        let base = tmp("stamp");
        assert_eq!(stamp(&base), "0:0");
        fs::write(base.join("2026-10-14.md"), "### 1\n").unwrap();
        fs::write(base.join("notes.md"), "ignored").unwrap();
        let one = stamp(&base);
        assert!(one.starts_with("1:"), "{one}");
        fs::write(base.join("2026-10-13.md"), "### 1\n").unwrap();
        assert!(stamp(&base).starts_with("2:"));
    }

    #[test]
    fn the_rollup_answer_is_read_leniently() {
        let wrapped = "Here you go:\n```json\n{\"decisions\":[{\"text\":\"Use WebGL\",\"date\":\"2026-10-14\"},\"plain string\"],\"open_questions\":[],\"topics\":[{\"name\":\"tunnel\",\"summary\":\"s\"},\"bare\"]}\n```";
        let b = parse_rollup(wrapped).unwrap();
        assert_eq!(b.decisions[0], Item { text: "Use WebGL".into(), date: Some("2026-10-14".into()) });
        assert_eq!(b.decisions[1], Item { text: "plain string".into(), date: None });
        assert_eq!(b.topics[1], Topic { name: "bare".into(), summary: String::new() });
        assert!(parse_rollup("no json here").is_err());
        assert_eq!(parse_rollup("{}").unwrap(), RollupBody::default());
    }

    #[test]
    fn memsearchs_config_is_read() {
        let c = parse_config(
            "# Resolved (all sources merged)\n[embedding]\nprovider = \"onnx\"\nmodel = \"bge\"\n[compact]\nllm_model = \"deepseek\"\nbase_url = \"http://127.0.0.1:8899/v1\"\napi_key = \"k\"\n",
        );
        assert_eq!(c.provider.as_deref(), Some("onnx"));
        assert_eq!(c.llm_base.as_deref(), Some("http://127.0.0.1:8899/v1"));
        assert_eq!(c.llm_key.as_deref(), Some("k"));
        assert!(parse_config("not toml [[[").llm_model.is_none());
    }

    #[test]
    fn a_job_runs_once_at_a_time() {
        let jobs = Mutex::new(HashMap::new());
        assert!(begin(&jobs, "c").is_ok());
        assert!(begin(&jobs, "c").is_err(), "already running");
        assert!(begin(&jobs, "other").is_ok());
        finish(&jobs, "c", Err(anyhow!("boom")));
        let j = job(&jobs, "c");
        assert!(!j.running);
        assert_eq!(j.error.as_deref(), Some("boom"));
        assert!(begin(&jobs, "c").is_ok(), "free again");
    }

    #[test]
    fn the_prompt_notes_are_bounded_and_newest_first() {
        let base = tmp("prompt");
        fs::write(base.join("2026-10-13.md"), "### 1\n- old\n").unwrap();
        fs::write(base.join("2026-10-14.md"), format!("### 1\n<!-- session:s -->\n- {}\n", "é".repeat(ROLLUP_CHARS))).unwrap();
        let notes = notes_for_prompt(&base);
        assert!(notes.starts_with("## 2026-10-14"));
        assert!(notes.len() <= ROLLUP_CHARS);
        assert!(!notes.contains("<!--"));
    }
}
