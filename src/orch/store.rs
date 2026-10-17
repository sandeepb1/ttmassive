//! Flows and runs on disk: one JSON file each under `<data_dir>/orch/`. Written whole through a
//! temp file so a crash never leaves half a document. Writes are small and synchronous, because the
//! executor saves from inside its change callback.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::graph::{Flow, id_ok};
use super::run::Run;

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// What the run list shows, without every transcript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: String,
    /// The flow this is an execution of. It may no longer be saved (deleted, or run before it was saved).
    pub flow_id: String,
    pub flow_name: String,
    pub goal: String,
    pub status: super::run::RunStatus,
    pub created: i64,
    /// Everything the agents read and wrote, cache included.
    pub tokens: u64,
    /// Dollars charged by OpenRouter for the run's agents on it.
    pub cost_usd: f64,
    /// Dollars its web access cost.
    pub web_usd: f64,
    /// The directory it ran in.
    pub project: String,
}

/// What `folders.json` holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct FolderFile {
    #[serde(default)]
    folders: Vec<String>,
    /// Folder path to the directory it is linked to.
    #[serde(default)]
    dirs: BTreeMap<String, String>,
}

/// A directory as typed (`~/code/app` is fine): it must exist and be a directory. Returns it absolute, with links resolved.
fn resolve_dir(text: &str) -> Result<String> {
    let t = text.trim();
    let expanded = match t.strip_prefix("~/").or(if t == "~" { Some("") } else { None }) {
        Some(rest) => crate::agentlog::home().context("there is no home folder to start from")?.join(rest),
        None => PathBuf::from(t),
    };
    if !expanded.is_absolute() {
        bail!("give the directory's full path, or start it with ~/");
    }
    let real = fs::canonicalize(&expanded).map_err(|_| anyhow::anyhow!("there is no directory at {t}"))?;
    if !real.is_dir() {
        bail!("{t} is not a directory");
    }
    Ok(real.to_string_lossy().into_owned())
}

/// Whether `path` is `folder` or inside it.
fn in_folder(path: &str, folder: &str) -> bool {
    path == folder || path.starts_with(&format!("{folder}/"))
}

/// `path`, which is `from` or inside it, with `from` replaced by `to`.
fn reparent(path: &str, from: &str, to: &str) -> String {
    format!("{to}{}", &path[from.len()..])
}

impl Store {
    pub fn new(data_dir: &Path) -> Store {
        Store { root: data_dir.join("orch") }
    }

    pub fn worktrees(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    fn file(&self, kind: &str, id: &str) -> Result<PathBuf> {
        if !id_ok(id) {
            bail!("bad id");
        }
        Ok(self.root.join(kind).join(format!("{id}.json")))
    }

    fn write<T: Serialize>(&self, kind: &str, id: &str, v: &T) -> Result<()> {
        let path = self.file(kind, id)?;
        fs::create_dir_all(path.parent().context("no parent")?)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Every readable document of a kind. An unreadable one is skipped, not fatal.
    fn read_all<T: serde::de::DeserializeOwned>(&self, kind: &str) -> Vec<T> {
        let Ok(dir) = fs::read_dir(self.root.join(kind)) else { return Vec::new() };
        dir.filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice(&fs::read(e.path()).ok()?).ok())
            .collect()
    }

    pub fn save_flow(&self, f: &Flow) -> Result<()> {
        f.validate().map_err(anyhow::Error::msg)?;
        self.write("flows", &f.id, f)
    }

    pub fn load_flow(&self, id: &str) -> Option<Flow> {
        serde_json::from_slice(&fs::read(self.file("flows", id).ok()?).ok()?).ok()
    }

    pub fn list_flows(&self) -> Vec<Flow> {
        let mut v: Vec<Flow> = self.read_all("flows");
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        v
    }

    /// Folders made on purpose, and which of them are linked to a directory.
    fn read_folder_file(&self) -> FolderFile {
        fs::read(self.root.join("folders.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn write_folder_file(&self, f: &FolderFile) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let path = self.root.join("folders.json");
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(f)?)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Every folder there is: those made on purpose (so an empty one stays), those flows are kept in, and the folders above
    /// them. Sorted, so a folder comes before what is inside it.
    pub fn list_folders(&self) -> Vec<String> {
        let mut all = std::collections::BTreeSet::new();
        for path in self.read_folder_file().folders.into_iter().chain(self.list_flows().into_iter().map(|f| f.folder)) {
            let mut at = String::new();
            for name in path.split('/').filter(|n| !n.is_empty()) {
                if !at.is_empty() {
                    at.push('/');
                }
                at.push_str(name);
                all.insert(at.clone());
            }
        }
        all.into_iter().collect()
    }

    /// The folders that are linked to a directory, and the directory each is linked to.
    pub fn folder_dirs(&self) -> BTreeMap<String, String> {
        let exists: std::collections::HashSet<String> = self.list_folders().into_iter().collect();
        self.read_folder_file().dirs.into_iter().filter(|(f, _)| exists.contains(f)).collect()
    }

    /// The directory a flow kept in `folder` is tied to: that of the folder itself, or else of the nearest folder above it that
    /// is linked. Returns that folder too. `None` when no folder from there up is linked, which leaves a run's directory free.
    pub fn linked_dir(&self, folder: &str) -> Option<(String, String)> {
        let dirs = self.folder_dirs();
        let mut at = folder.to_owned();
        loop {
            if let Some(d) = dirs.get(&at) {
                return Some((at, d.clone()));
            }
            if at.is_empty() {
                return None;
            }
            at = at.rsplit_once('/').map_or(String::new(), |(parent, _)| parent.to_owned());
        }
    }

    /// Make a folder (and the ones above it), linked to `dir` when one is given. Returns its tidy path.
    pub fn create_folder(&self, path: &str, dir: Option<&str>) -> Result<String> {
        let path = super::graph::clean_folder(path).map_err(anyhow::Error::msg)?;
        if path.is_empty() {
            bail!("give the folder a name");
        }
        let mut file = self.read_folder_file();
        for p in self.list_folders() {
            if !file.folders.contains(&p) {
                file.folders.push(p);
            }
        }
        if !file.folders.contains(&path) {
            file.folders.push(path.clone());
        }
        self.write_folder_file(&file)?;
        if let Some(d) = dir.filter(|d| !d.trim().is_empty()) {
            if let Err(e) = self.link_folder(&path, Some(d)) {
                // A folder that cannot have the directory it was made for is not left behind half made.
                let _ = self.delete_folder(&path);
                return Err(e);
            }
        }
        Ok(path)
    }

    /// Link a folder at the top level to a directory, or take the link off with `None`. The directory must exist. Returns the
    /// directory as it is kept: absolute, with symbolic links resolved.
    pub fn link_folder(&self, path: &str, dir: Option<&str>) -> Result<Option<String>> {
        let path = super::graph::clean_folder(path).map_err(anyhow::Error::msg)?;
        if path.is_empty() || !self.list_folders().contains(&path) {
            bail!("there is no such folder");
        }
        let mut file = self.read_folder_file();
        let Some(dir) = dir.filter(|d| !d.trim().is_empty()) else {
            file.dirs.remove(&path);
            self.write_folder_file(&file)?;
            return Ok(None);
        };
        // A directory is a folder at the top level; the folders inside it only organise, and their flows run in its directory.
        if path.contains('/') {
            bail!("only a folder at the top level can be linked to a directory: the folders inside it run in its directory");
        }
        let dir = resolve_dir(dir)?;
        file.dirs.insert(path, dir.clone());
        self.write_folder_file(&file)?;
        Ok(Some(dir))
    }

    /// Rename a folder, which moves what is inside it with it. Returns how many flows moved. Refuses a name that is taken, or
    /// a place inside the folder itself.
    pub fn rename_folder(&self, from: &str, to: &str) -> Result<usize> {
        let (from, to) = (super::graph::clean_folder(from).map_err(anyhow::Error::msg)?, super::graph::clean_folder(to).map_err(anyhow::Error::msg)?);
        if from.is_empty() || to.is_empty() {
            bail!("a folder needs a name");
        }
        let all = self.list_folders();
        if !all.contains(&from) {
            bail!("there is no such folder");
        }
        if to == from || to.starts_with(&format!("{from}/")) {
            bail!("a folder cannot be moved into itself");
        }
        if all.contains(&to) {
            bail!("there is already a folder called that");
        }
        let flows: Vec<Flow> = self.list_flows().into_iter().filter(|f| in_folder(&f.folder, &from)).collect();
        // Check every new place before changing anything.
        for f in &flows {
            let mut moved = f.clone();
            moved.folder = reparent(&f.folder, &from, &to);
            moved.validate().map_err(anyhow::Error::msg)?;
        }
        let folders: Vec<String> = all.iter().map(|p| if in_folder(p, &from) { reparent(p, &from, &to) } else { p.clone() }).collect();
        for p in &folders {
            super::graph::clean_folder(p).map_err(anyhow::Error::msg)?;
        }
        // A link goes with its folder, which must stay at the top level for it to keep one.
        let mut file = self.read_folder_file();
        file.dirs = file.dirs.into_iter().map(|(f, d)| (if in_folder(&f, &from) { reparent(&f, &from, &to) } else { f }, d)).collect();
        if let Some(f) = file.dirs.keys().find(|f| f.contains('/')) {
            bail!("“{f}” is linked to a directory, so it must stay at the top level");
        }
        for f in &flows {
            let mut moved = f.clone();
            moved.folder = reparent(&f.folder, &from, &to);
            self.save_flow(&moved)?;
        }
        file.folders = folders;
        self.write_folder_file(&file)?;
        Ok(flows.len())
    }

    /// Remove a folder that has no flows in it, with any empty folders inside it, and their links.
    pub fn delete_folder(&self, path: &str) -> Result<()> {
        let path = super::graph::clean_folder(path).map_err(anyhow::Error::msg)?;
        if path.is_empty() {
            bail!("a folder needs a name");
        }
        if self.list_flows().iter().any(|f| in_folder(&f.folder, &path)) {
            bail!("the folder has flows in it: move or delete them first");
        }
        let mut file = self.read_folder_file();
        file.folders = self.list_folders().into_iter().filter(|p| !in_folder(p, &path)).collect();
        file.dirs.retain(|f, _| !in_folder(f, &path));
        self.write_folder_file(&file)
    }

    /// Put a flow in a folder (`""` for the top level). The folder is made if it is new.
    pub fn move_flow(&self, id: &str, folder: &str) -> Result<()> {
        let mut flow = self.load_flow(id).context("there is no such flow")?;
        flow.folder = super::graph::clean_folder(folder).map_err(anyhow::Error::msg)?;
        self.save_flow(&flow)
    }

    pub fn delete_flow(&self, id: &str) -> Result<()> {
        match fs::remove_file(self.file("flows", id)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// The plan limits, or the defaults when none were saved (or the file cannot be read).
    pub fn load_limits(&self) -> super::graph::Budget {
        std::fs::read(self.file("limits", "global").unwrap_or_default())
            .ok()
            .and_then(|b| serde_json::from_slice::<super::graph::Budget>(&b).ok())
            .filter(|b| b.validate().is_ok())
            .unwrap_or_default()
    }

    pub fn save_limits(&self, b: &super::graph::Budget) -> Result<()> {
        b.validate().map_err(anyhow::Error::msg)?;
        self.write("limits", "global", b)
    }

    pub fn save_run(&self, r: &Run) -> Result<()> {
        self.write("runs", &r.id, r)
    }

    pub fn load_run(&self, id: &str) -> Option<Run> {
        serde_json::from_slice(&fs::read(self.file("runs", id).ok()?).ok()?).ok()
    }

    fn turns_file(&self, id: &str) -> Result<PathBuf> {
        if !id_ok(id) {
            bail!("bad id");
        }
        Ok(self.root.join("runs").join(format!("{id}.turns.jsonl")))
    }

    /// Keep the full record of a finished turn: one line in the run's own file, added to as the run goes on.
    pub fn append_turn(&self, run_id: &str, d: &super::run::TurnDetail) -> Result<()> {
        use std::io::Write;
        let path = self.turns_file(run_id)?;
        fs::create_dir_all(path.parent().context("no parent")?)?;
        let mut f = fs::OpenOptions::new().create(true).append(true).open(path)?;
        let mut line = serde_json::to_vec(d)?;
        line.push(b'\n');
        f.write_all(&line)?;
        Ok(())
    }

    /// The full record of turn `n`, if it was kept (a run from before turns were recorded has none).
    pub fn read_turn(&self, run_id: &str, n: u32) -> Option<super::run::TurnDetail> {
        let text = fs::read_to_string(self.turns_file(run_id).ok()?).ok()?;
        text.lines().filter_map(|l| serde_json::from_str::<super::run::TurnDetail>(l).ok()).find(|d| d.n == n)
    }

    pub fn delete_run(&self, id: &str) -> Result<()> {
        let _ = fs::remove_file(self.turns_file(id)?);
        match fs::remove_file(self.file("runs", id)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub fn all_runs(&self) -> Vec<Run> {
        self.read_all("runs")
    }

    /// Newest first.
    pub fn list_runs(&self) -> Vec<RunSummary> {
        let mut v: Vec<RunSummary> = self
            .all_runs()
            .into_iter()
            .map(|r| {
                let tokens = r.tokens().total();
                let (cost_usd, web_usd) = (r.cost_usd(), r.web_usd());
                RunSummary { id: r.id, flow_id: r.flow.id, flow_name: r.flow.name, goal: r.goal, status: r.status, created: r.created, tokens, cost_usd, web_usd, project: r.project }
            })
            .collect();
        v.sort_by(|a, b| b.created.cmp(&a.created));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orch::run::RunStatus;
    use crate::orch::starters;

    /// A fresh temp folder that is removed when the test ends, pass or fail.
    struct Tmp(PathBuf);

    impl std::ops::Deref for Tmp {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(name: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("ttm-orch-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    #[test]
    fn a_flow_is_saved_listed_loaded_and_deleted() {
        let dir = tmp("flow");
        let s = Store::new(&dir);
        assert!(s.list_flows().is_empty());
        let f = starters::all().remove(0);
        s.save_flow(&f).unwrap();
        assert_eq!(s.load_flow(&f.id), Some(f.clone()));
        assert_eq!(s.list_flows().len(), 1);
        s.delete_flow(&f.id).unwrap();
        assert_eq!(s.load_flow(&f.id), None);
        s.delete_flow(&f.id).unwrap(); // already gone is fine
    }

    #[test]
    fn an_invalid_flow_is_not_saved() {
        let dir = tmp("invalid");
        let s = Store::new(&dir);
        let mut f = starters::all().remove(0);
        f.nodes.clear();
        assert!(s.save_flow(&f).is_err());
        assert!(s.list_flows().is_empty());
    }

    #[test]
    fn ids_cannot_escape_the_folder() {
        let dir = tmp("ids");
        let s = Store::new(&dir);
        assert!(s.file("runs", "../../etc/passwd").is_err());
        assert!(s.file("runs", "a/b").is_err());
        assert!(s.file("runs", "").is_err());
        assert!(s.load_run("../x").is_none());
    }

    #[test]
    fn limits_default_until_saved_and_bad_ones_are_refused() {
        let dir = tmp("limits");
        let s = Store::new(&dir);
        assert_eq!(s.load_limits(), crate::orch::graph::Budget::default());
        let b = crate::orch::graph::Budget { claude_pct: 40.0, codex_pct: 30.0, stop_window_pct: 90.0, run_usd: Some(2.5), run_web_usd: Some(1.25) };
        s.save_limits(&b).unwrap();
        assert_eq!(s.load_limits(), b);
        assert!(s.save_limits(&crate::orch::graph::Budget { claude_pct: 0.0, ..b }).is_err());
        assert!(s.save_limits(&crate::orch::graph::Budget { codex_pct: 101.0, ..b }).is_err());
        assert_eq!(s.load_limits(), b, "a refused save leaves the old limits");
    }

    #[test]
    fn a_credit_cap_saved_before_web_access_was_priced_is_carried_over_in_dollars() {
        let dir = tmp("limits-old");
        let s = Store::new(&dir);
        let old = r#"{"claudePct":40.0,"codexPct":30.0,"stopWindowPct":90.0,"runUsd":2.5,"runCredits":120}"#;
        let b: crate::orch::graph::Budget = serde_json::from_str(old).unwrap();
        assert_eq!(b.run_web_usd, Some(0.0996), "120 Firecrawl credits at list price");
        assert_eq!(b.run_usd, Some(2.5));
        let both: crate::orch::graph::Budget = serde_json::from_str(r#"{"claudePct":40.0,"codexPct":30.0,"stopWindowPct":90.0,"runCredits":120,"runWebUsd":3.0}"#).unwrap();
        assert_eq!(both.run_web_usd, Some(3.0), "a dollar cap wins over the old credit count");
        let none: crate::orch::graph::Budget = serde_json::from_str(r#"{"claudePct":40.0,"codexPct":30.0,"stopWindowPct":90.0}"#).unwrap();
        assert_eq!(none.run_web_usd, None);
        s.save_limits(&b).unwrap();
        let saved = serde_json::to_string(&s.load_limits()).unwrap();
        assert!(saved.contains("runWebUsd") && !saved.contains("runCredits"), "{saved}");
    }

    #[test]
    fn a_deleted_run_is_gone_and_deleting_twice_is_fine() {
        let dir = tmp("rundel");
        let s = Store::new(&dir);
        let flow = starters::all().remove(0);
        s.save_run(&Run::new("a".into(), flow.clone(), "x".into(), 1)).unwrap();
        s.save_run(&Run::new("b".into(), flow, "y".into(), 2)).unwrap();
        s.delete_run("a").unwrap();
        s.delete_run("a").unwrap();
        assert!(s.load_run("a").is_none());
        assert_eq!(s.list_runs().len(), 1);
        assert!(s.delete_run("../b").is_err());
    }

    #[test]
    fn runs_round_trip_and_list_newest_first() {
        let dir = tmp("runs");
        let s = Store::new(&dir);
        let flow = starters::all().remove(0);
        let mut old = Run::new("old".into(), flow.clone(), "first".into(), 10);
        old.status = RunStatus::Done;
        let new = Run::new("new".into(), flow, "second".into(), 20);
        s.save_run(&old).unwrap();
        s.save_run(&new).unwrap();
        assert_eq!(s.load_run("new"), Some(new));
        let list = s.list_runs();
        assert_eq!(list.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["new", "old"]);
        assert_eq!(list[1].goal, "first");
        assert_eq!(list[0].flow_id, "starter-feature", "a run says which flow it belongs to");
        assert_eq!(list[0].tokens, 0);
    }

    #[test]
    fn the_summary_carries_the_flow_id_and_token_total_under_the_names_the_page_reads() {
        let dir = tmp("summary");
        let s = Store::new(&dir);
        let mut r = Run::new("r1".into(), starters::all().remove(0), "g".into(), 5);
        r.overhead = crate::orch::usage::Tokens { input: 1, output: 2, cache_read: 3, cache_write: 4 };
        s.save_run(&r).unwrap();
        let v = serde_json::to_value(s.list_runs()).unwrap();
        assert_eq!(v[0]["flowId"], "starter-feature");
        assert_eq!(v[0]["tokens"], 10);
    }

    #[test]
    fn a_corrupt_file_is_skipped() {
        let dir = tmp("corrupt");
        let s = Store::new(&dir);
        s.save_flow(&starters::all().remove(0)).unwrap();
        fs::write(dir.join("orch/flows/broken.json"), "{ nope").unwrap();
        assert_eq!(s.list_flows().len(), 1);
    }

    fn flow_in(id: &str, folder: &str) -> Flow {
        let mut f = starters::all().remove(0);
        f.id = id.into();
        f.name = id.into();
        f.folder = folder.into();
        f
    }

    #[test]
    fn folders_are_those_made_and_those_flows_sit_in_with_the_ones_above() {
        let dir = tmp("folders");
        let s = Store::new(&dir.0);
        assert!(s.list_folders().is_empty());
        s.save_flow(&flow_in("a", "Work/Bugs")).unwrap();
        assert_eq!(s.list_folders(), ["Work", "Work/Bugs"], "a folder a flow is in brings the ones above it");
        assert_eq!(s.create_folder(" Personal / Notes ", None).unwrap(), "Personal/Notes");
        assert_eq!(s.list_folders(), ["Personal", "Personal/Notes", "Work", "Work/Bugs"]);
        assert!(s.create_folder("", None).is_err() && s.create_folder("a/b/c/d/e", None).is_err());
        assert_eq!(s.create_folder("Work", None).unwrap(), "Work", "making one that exists is fine");
    }

    #[test]
    fn renaming_a_folder_moves_the_flows_and_folders_inside_it() {
        let dir = tmp("rename");
        let s = Store::new(&dir.0);
        s.save_flow(&flow_in("a", "Work")).unwrap();
        s.save_flow(&flow_in("b", "Work/Bugs")).unwrap();
        s.save_flow(&flow_in("c", "")).unwrap();
        s.create_folder("Work/Empty", None).unwrap();
        assert_eq!(s.rename_folder("Work", "Jobs").unwrap(), 2);
        let at = |id: &str| s.load_flow(id).unwrap().folder;
        assert_eq!((at("a"), at("b"), at("c")), ("Jobs".into(), "Jobs/Bugs".into(), String::new()));
        assert_eq!(s.list_folders(), ["Jobs", "Jobs/Bugs", "Jobs/Empty"]);
        assert!(s.rename_folder("Jobs", "Jobs/Inside").is_err(), "not into itself");
        s.create_folder("Taken", None).unwrap();
        assert!(s.rename_folder("Jobs", "Taken").is_err(), "not onto another");
        assert!(s.rename_folder("Nope", "X").is_err());
        assert_eq!(at("a"), "Jobs", "a refused rename changes nothing");
    }

    #[test]
    fn a_folder_with_flows_in_it_is_not_deleted_and_an_empty_one_goes_with_what_is_inside() {
        let dir = tmp("fdel");
        let s = Store::new(&dir.0);
        s.save_flow(&flow_in("a", "Work/Bugs")).unwrap();
        s.create_folder("Work/Empty/Deeper", None).unwrap();
        assert!(s.delete_folder("Work").unwrap_err().to_string().contains("flows in it"));
        assert!(s.list_folders().contains(&"Work/Empty/Deeper".to_string()), "nothing was removed");
        s.delete_folder("Work/Empty").unwrap();
        assert_eq!(s.list_folders(), ["Work", "Work/Bugs"]);
        s.move_flow("a", "").unwrap();
        s.delete_folder("Work").unwrap();
        assert!(s.list_folders().is_empty());
    }

    #[test]
    fn a_flow_is_moved_to_a_folder_and_back_to_the_top() {
        let dir = tmp("fmove");
        let s = Store::new(&dir.0);
        s.save_flow(&flow_in("a", "")).unwrap();
        s.move_flow("a", " Work / Bugs ").unwrap();
        assert_eq!(s.load_flow("a").unwrap().folder, "Work/Bugs");
        assert_eq!(s.list_folders(), ["Work", "Work/Bugs"]);
        s.move_flow("a", "").unwrap();
        assert_eq!(s.load_flow("a").unwrap().folder, "");
        assert!(s.move_flow("nope", "X").is_err());
        assert!(s.move_flow("a", "a/b/c/d/e").is_err());
    }

    /// A store, and a folder tree of real directories to link to: `code/alpha`, `code/beta` and `other`.
    fn linked_store(name: &str) -> (Store, Tmp, PathBuf) {
        let dir = tmp(name);
        let code = dir.0.join("code");
        for sub in ["code/alpha", "code/beta", "other"] {
            fs::create_dir_all(dir.0.join(sub)).unwrap();
        }
        let store = Store::new(&dir.0.join("data"));
        let code = fs::canonicalize(code).unwrap();
        (store, dir, code)
    }

    fn text(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn a_folder_is_linked_to_a_real_directory_which_is_kept_in_full() {
        let (s, _d, code) = linked_store("link");
        assert_eq!(s.create_folder("Code", Some(&text(&code))).unwrap(), "Code");
        assert_eq!(s.folder_dirs().get("Code"), Some(&text(&code)));
        assert!(s.link_folder("Code", Some("/no/such/place")).unwrap_err().to_string().contains("no directory at /no/such/place"));
        assert!(s.link_folder("Code", Some("relative/dir")).unwrap_err().to_string().contains("full path"));
        let file = code.join("alpha/not-a-dir");
        fs::write(&file, "x").unwrap();
        assert!(s.link_folder("Code", Some(&text(&file))).unwrap_err().to_string().contains("not a directory"));
        assert_eq!(s.folder_dirs().get("Code"), Some(&text(&code)), "a refused link leaves the old one");
        assert_eq!(s.link_folder("Code", None).unwrap(), None);
        assert!(s.folder_dirs().is_empty(), "the link comes off and the folder stays");
        assert!(s.list_folders().contains(&"Code".to_string()));
        assert!(s.link_folder("Nope", Some(&text(&code))).is_err(), "only a folder that exists");
    }

    #[test]
    fn a_link_may_be_typed_from_the_home_folder() {
        let Some(home) = crate::agentlog::home() else { return };
        let (s, _d, _) = linked_store("home");
        s.create_folder("Home", Some("~")).unwrap();
        assert_eq!(s.folder_dirs().get("Home"), Some(&text(&fs::canonicalize(home).unwrap())));
    }

    #[test]
    fn a_folder_with_a_directory_it_cannot_have_is_not_left_behind() {
        let (s, _d, _) = linked_store("half");
        assert!(s.create_folder("Ghost", Some("/no/such/place")).is_err());
        assert!(!s.list_folders().contains(&"Ghost".to_string()));
        assert!(s.folder_dirs().is_empty());
    }

    #[test]
    fn a_flow_is_tied_to_the_directory_of_the_top_level_folder_it_is_kept_in() {
        let (s, _d, code) = linked_store("near");
        s.create_folder("Code", Some(&text(&code))).unwrap();
        s.create_folder("Code/notes/deep", None).unwrap();
        s.create_folder("Loose", None).unwrap();
        let near = |f: &str| s.linked_dir(f);
        assert_eq!(near("Code"), Some(("Code".into(), text(&code))));
        assert_eq!(near("Code/notes"), Some(("Code".into(), text(&code))), "a folder inside it runs in its directory");
        assert_eq!(near("Code/notes/deep"), Some(("Code".into(), text(&code))));
        assert_eq!(near("Loose"), None, "a folder that is not linked leaves a run free");
        assert_eq!(near(""), None);
    }

    #[test]
    fn only_a_folder_at_the_top_level_can_be_linked_to_a_directory() {
        let (s, _d, code) = linked_store("top");
        s.create_folder("Code", Some(&text(&code))).unwrap();
        let why = s.create_folder("Code/alpha", Some(&text(&code.join("alpha")))).unwrap_err().to_string();
        assert!(why.contains("only a folder at the top level"), "{why}");
        assert!(!s.list_folders().contains(&"Code/alpha".to_string()), "no half-made folder");
        s.create_folder("Code/plain", None).unwrap();
        assert!(s.link_folder("Code/plain", Some(&text(&code))).is_err());
        assert_eq!(s.folder_dirs().len(), 1);
    }

    #[test]
    fn a_link_goes_with_its_folder_when_it_is_renamed_and_goes_when_it_is_deleted() {
        let (s, _d, code) = linked_store("moves");
        s.create_folder("Code", Some(&text(&code))).unwrap();
        s.create_folder("Code/notes", None).unwrap();
        s.rename_folder("Code", "Work").unwrap();
        let dirs = s.folder_dirs();
        assert_eq!((dirs.get("Work"), dirs.get("Code")), (Some(&text(&code)), None), "{dirs:?}");
        assert!(s.list_folders().contains(&"Work/notes".to_string()));
        s.delete_folder("Work").unwrap();
        assert!(s.folder_dirs().is_empty(), "nothing is left pointing at a folder that is gone");
    }

    #[test]
    fn a_linked_folder_cannot_be_moved_below_the_top_level() {
        let (s, _d, code) = linked_store("below");
        s.create_folder("Code", Some(&text(&code))).unwrap();
        s.create_folder("Group", None).unwrap();
        let why = s.rename_folder("Code", "Group/Code").unwrap_err().to_string();
        assert!(why.contains("must stay at the top level"), "{why}");
        assert!(s.folder_dirs().contains_key("Code") && s.list_folders().contains(&"Code".to_string()), "nothing moved");
        s.create_folder("Group/plain", None).unwrap();
        s.rename_folder("Group", "Renamed").unwrap();
        assert!(s.list_folders().contains(&"Renamed/plain".to_string()), "a folder with no link can go anywhere");
    }

    #[test]
    fn a_folder_that_is_not_linked_has_no_directory_and_the_file_survives_a_reread() {
        let (s, d, code) = linked_store("reread");
        s.create_folder("Plain", None).unwrap();
        s.create_folder("Code", Some(&text(&code))).unwrap();
        let again = Store::new(&d.0.join("data"));
        assert_eq!(again.folder_dirs().len(), 1);
        assert_eq!(again.list_folders(), ["Code", "Plain"]);
    }
}
