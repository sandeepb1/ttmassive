//! Flows and runs on disk: one JSON file each under `<data_dir>/orch/`. Written whole through a
//! temp file so a crash never leaves half a document. Writes are small and synchronous, because the
//! executor saves from inside its change callback.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

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

    /// Every folder there is: those made on purpose (so an empty one stays), those flows are kept in, and the folders above
    /// them. Sorted, so a folder comes before what is inside it.
    pub fn list_folders(&self) -> Vec<String> {
        let declared: Vec<String> = fs::read(self.root.join("folders.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut all = std::collections::BTreeSet::new();
        for path in declared.into_iter().chain(self.list_flows().into_iter().map(|f| f.folder)) {
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

    fn save_declared(&self, folders: &[String]) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let path = self.root.join("folders.json");
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(folders)?)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Make a folder (and the ones above it). Returns its tidy path.
    pub fn create_folder(&self, path: &str) -> Result<String> {
        let path = super::graph::clean_folder(path).map_err(anyhow::Error::msg)?;
        if path.is_empty() {
            bail!("give the folder a name");
        }
        let mut declared = self.list_folders();
        if !declared.contains(&path) {
            declared.push(path.clone());
        }
        self.save_declared(&declared)?;
        Ok(path)
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
        let declared: Vec<String> = all.iter().map(|p| if in_folder(p, &from) { reparent(p, &from, &to) } else { p.clone() }).collect();
        for p in &declared {
            super::graph::clean_folder(p).map_err(anyhow::Error::msg)?;
        }
        for f in &flows {
            let mut moved = f.clone();
            moved.folder = reparent(&f.folder, &from, &to);
            self.save_flow(&moved)?;
        }
        self.save_declared(&declared)?;
        Ok(flows.len())
    }

    /// Remove a folder that has no flows in it, with any empty folders inside it.
    pub fn delete_folder(&self, path: &str) -> Result<()> {
        let path = super::graph::clean_folder(path).map_err(anyhow::Error::msg)?;
        if path.is_empty() {
            bail!("a folder needs a name");
        }
        if self.list_flows().iter().any(|f| in_folder(&f.folder, &path)) {
            bail!("the folder has flows in it: move or delete them first");
        }
        let kept: Vec<String> = self.list_folders().into_iter().filter(|p| !in_folder(p, &path)).collect();
        self.save_declared(&kept)
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
                RunSummary { id: r.id, flow_id: r.flow.id, flow_name: r.flow.name, goal: r.goal, status: r.status, created: r.created, tokens, cost_usd, web_usd }
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
        assert_eq!(s.create_folder(" Personal / Notes ").unwrap(), "Personal/Notes");
        assert_eq!(s.list_folders(), ["Personal", "Personal/Notes", "Work", "Work/Bugs"]);
        assert!(s.create_folder("").is_err() && s.create_folder("a/b/c/d/e").is_err());
        assert_eq!(s.create_folder("Work").unwrap(), "Work", "making one that exists is fine");
    }

    #[test]
    fn renaming_a_folder_moves_the_flows_and_folders_inside_it() {
        let dir = tmp("rename");
        let s = Store::new(&dir.0);
        s.save_flow(&flow_in("a", "Work")).unwrap();
        s.save_flow(&flow_in("b", "Work/Bugs")).unwrap();
        s.save_flow(&flow_in("c", "")).unwrap();
        s.create_folder("Work/Empty").unwrap();
        assert_eq!(s.rename_folder("Work", "Jobs").unwrap(), 2);
        let at = |id: &str| s.load_flow(id).unwrap().folder;
        assert_eq!((at("a"), at("b"), at("c")), ("Jobs".into(), "Jobs/Bugs".into(), String::new()));
        assert_eq!(s.list_folders(), ["Jobs", "Jobs/Bugs", "Jobs/Empty"]);
        assert!(s.rename_folder("Jobs", "Jobs/Inside").is_err(), "not into itself");
        s.create_folder("Taken").unwrap();
        assert!(s.rename_folder("Jobs", "Taken").is_err(), "not onto another");
        assert!(s.rename_folder("Nope", "X").is_err());
        assert_eq!(at("a"), "Jobs", "a refused rename changes nothing");
    }

    #[test]
    fn a_folder_with_flows_in_it_is_not_deleted_and_an_empty_one_goes_with_what_is_inside() {
        let dir = tmp("fdel");
        let s = Store::new(&dir.0);
        s.save_flow(&flow_in("a", "Work/Bugs")).unwrap();
        s.create_folder("Work/Empty/Deeper").unwrap();
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
}
