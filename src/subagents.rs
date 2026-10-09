//! Helper agents: the Claude `Agent` tool and Codex's spawned threads. Each has a log of its own,
//! so the web UI can show what it is doing and open its whole transcript.
//!
//! Claude keeps them in `<conversation>/subagents/agent-<id>.jsonl` with a `.meta.json` beside each
//! (its type, description and the id of the tool call that started it). Codex writes each helper
//! as a rollout of its own, and the parent conversation's log records when it started and ended.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::Value;

use crate::{
    agentlog,
    chat::{ChatState, Spawn},
    config::Agent,
    uploads::valid_component,
};

/// A helper whose log has been written to within this long counts as running.
pub const RUNNING_SECS: u64 = 90;
/// Helpers listed per conversation.
const MAX_AGENTS: usize = 60;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubAgent {
    pub id: String,
    /// Claude's agent type (`Explore`, `general-purpose`), or `codex`.
    pub kind: String,
    pub description: String,
    /// The id of the tool call (or log item) in the parent conversation that started it.
    pub tool_use_id: Option<String>,
    /// 1 for a helper of the main conversation, 2 for a helper of a helper, and so on.
    pub depth: u32,
    /// The helper that started this one, when it was not the main conversation.
    pub parent: Option<String>,
    pub running: bool,
    /// Unix seconds of its first log line.
    pub started: Option<i64>,
    /// Unix seconds its log last changed.
    pub last: Option<i64>,
    pub tools: u32,
    pub last_action: Option<String>,
    /// The last thing it said: its report, once it is done.
    pub result: Option<String>,
    /// Its log file exists (a helper that has only just started may not have one yet).
    pub has_log: bool,
}

pub type States = HashMap<String, ChatState>;

/// The key under which a helper's parsed log is kept beside its parent's.
pub fn key(session: &str, agent_id: &str) -> String {
    format!("{session}#{agent_id}")
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn mtime(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Every helper of `session`'s conversation, oldest first. Reads each helper's log incrementally;
/// the parsed logs live in `states` beside the parent's. Blocking.
///
/// `parent_log` is the conversation's own log (where Claude's helpers are found); `spawns` are the
/// helpers Codex's log announced; `codex_sessions` is where Codex keeps its rollouts.
pub fn collect(
    states: &mut States,
    session: &str,
    agent: Agent,
    parent_log: Option<&Path>,
    spawns: &[Spawn],
    codex_sessions: Option<&Path>,
    now: u64,
) -> Vec<SubAgent> {
    let mut out = match agent {
        Agent::Claude => parent_log.map(|p| claude_agents(states, session, p, now)).unwrap_or_default(),
        Agent::Codex => codex_agents(states, session, spawns, codex_sessions),
        Agent::Other => Vec::new(),
    };
    link_parents(states, session, &mut out);
    out.sort_by_key(|a| (a.started.unwrap_or(i64::MAX), a.id.clone()));
    out.truncate(MAX_AGENTS);
    out
}

fn summarize(states: &mut States, session: &str, id: &str, agent: Agent, log: &Path) -> (crate::chat::Summary, bool) {
    let st = states.entry(key(session, id)).or_insert_with(|| ChatState::new_sub(agent, log.to_path_buf()));
    let has_log = st.update().unwrap_or(false);
    (st.summary(), has_log)
}

fn claude_agents(states: &mut States, session: &str, parent_log: &Path, now: u64) -> Vec<SubAgent> {
    let dir = parent_log.with_extension("").join("subagents");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_prefix("agent-").and_then(|n| n.strip_suffix(".meta.json")) else { continue };
        if !valid_component(id) {
            continue;
        }
        let meta: Value = std::fs::read(entry.path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        let log = dir.join(format!("agent-{id}.jsonl"));
        let (sum, has_log) = summarize(states, session, id, Agent::Claude, &log);
        let last = mtime(&log);
        let text = |k: &str| meta[k].as_str().unwrap_or("").to_owned();
        out.push(SubAgent {
            id: id.to_owned(),
            kind: Some(text("agentType")).filter(|k| !k.is_empty()).unwrap_or_else(|| "agent".into()),
            description: text("description"),
            tool_use_id: Some(text("toolUseId")).filter(|t| !t.is_empty()),
            depth: meta["spawnDepth"].as_u64().unwrap_or(1) as u32,
            parent: None,
            running: has_log && last.is_some_and(|t| now.saturating_sub(t) < RUNNING_SECS),
            started: sum.first_ts,
            last: last.map(|t| t as i64),
            tools: sum.tools,
            last_action: sum.last_action,
            result: sum.result,
            has_log,
        });
    }
    out
}

fn codex_agents(states: &mut States, session: &str, spawns: &[Spawn], root: Option<&Path>) -> Vec<SubAgent> {
    let mut out = Vec::new();
    for sp in spawns {
        if !valid_component(&sp.thread_id) {
            continue;
        }
        let k = key(session, &sp.thread_id);
        // Once a helper's log has been found it is remembered by its parsed state; until then look.
        let log: Option<PathBuf> =
            states.get(&k).and_then(|s| s.path().map(Path::to_path_buf)).or_else(|| agentlog::find_rollout(root?, &sp.thread_id));
        let (sum, has_log, last) = match &log {
            Some(l) => {
                let (sum, has) = summarize(states, session, &sp.thread_id, Agent::Codex, l);
                (sum, has, mtime(l))
            }
            None => (Default::default(), false, None),
        };
        out.push(SubAgent {
            id: sp.thread_id.clone(),
            kind: "codex".into(),
            description: sp.name.replace('_', " "),
            tool_use_id: Some(sp.started_id.clone()),
            depth: sp.depth.max(1),
            parent: None,
            running: !sp.done,
            started: sum.first_ts,
            last: last.map(|t| t as i64),
            tools: sum.tools,
            last_action: sum.last_action,
            result: sum.result,
            has_log,
        });
    }
    out
}

/// A helper that was started from inside another helper's log has that helper as its parent.
fn link_parents(states: &States, session: &str, agents: &mut [SubAgent]) {
    let owners: Vec<(String, &ChatState)> = agents.iter().filter_map(|a| Some((a.id.clone(), states.get(&key(session, &a.id))?))).collect();
    for a in agents.iter_mut() {
        let Some(tool) = a.tool_use_id.as_deref() else { continue };
        a.parent = owners.iter().find(|(id, st)| *id != a.id && st.has_item(tool)).map(|(id, _)| id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ttm-sub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_lines(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(path, text).unwrap();
    }

    fn age(path: &Path, secs: u64) {
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
    }

    fn side_user(text: &str) -> Value {
        json!({ "type": "user", "uuid": "u0", "isSidechain": true, "timestamp": "2026-10-09T14:00:00.000Z", "message": { "role": "user", "content": text } })
    }
    fn side_tool(uuid: &str, id: &str, name: &str, input: Value) -> Value {
        json!({ "type": "assistant", "uuid": uuid, "isSidechain": true, "timestamp": "2026-10-09T14:00:05.000Z",
            "message": { "model": "m", "content": [{ "type": "tool_use", "id": id, "name": name, "input": input }] } })
    }
    fn side_say(uuid: &str, text: &str) -> Value {
        json!({ "type": "assistant", "uuid": uuid, "isSidechain": true, "timestamp": "2026-10-09T14:01:00.000Z",
            "message": { "model": "m", "content": [{ "type": "text", "text": text }] } })
    }

    /// `<root>/projects/p/conv.jsonl` and its `conv/subagents/` folder.
    fn claude_fixture(root: &Path) -> PathBuf {
        let parent = root.join("projects/p/conv.jsonl");
        write_lines(&parent, &[json!({ "type": "user", "uuid": "p1", "message": { "content": "hi" } })]);
        parent
    }

    fn add_claude_agent(parent: &Path, id: &str, meta: Value, lines: &[Value]) -> PathBuf {
        let dir = parent.with_extension("").join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("agent-{id}.meta.json")), meta.to_string()).unwrap();
        let log = dir.join(format!("agent-{id}.jsonl"));
        write_lines(&log, lines);
        log
    }

    #[test]
    fn claude_helpers_are_listed_with_what_they_did() {
        let root = temp("claude");
        let parent = claude_fixture(&root);
        let log = add_claude_agent(
            &parent,
            "a1",
            json!({ "agentType": "Explore", "description": "Find the bug", "toolUseId": "toolu_1", "spawnDepth": 1 }),
            &[
                side_user("Look for the bug"),
                side_tool("a", "t1", "Grep", json!({ "pattern": "retry" })),
                side_tool("b", "t2", "Read", json!({ "file_path": "/p/src/a.rs" })),
                side_say("c", "Found it in a.rs."),
            ],
        );
        let mut states = States::new();
        let now = now_secs();
        let list = collect(&mut states, "s", Agent::Claude, Some(&parent), &[], None, now);
        assert_eq!(list.len(), 1);
        let a = &list[0];
        assert_eq!((a.id.as_str(), a.kind.as_str(), a.description.as_str()), ("a1", "Explore", "Find the bug"));
        assert_eq!(a.tool_use_id.as_deref(), Some("toolu_1"));
        assert_eq!((a.depth, a.tools, a.has_log), (1, 2, true));
        assert_eq!(a.last_action.as_deref(), Some("Read src/a.rs"));
        assert_eq!(a.result.as_deref(), Some("Found it in a.rs."));
        assert!(a.started.is_some() && a.parent.is_none());
        assert!(a.running, "its log was just written");
        age(&log, 3600);
        let later = collect(&mut states, "s", Agent::Claude, Some(&parent), &[], None, now_secs());
        assert!(!later[0].running, "an hour of silence means it finished");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_helper_started_inside_another_has_it_as_parent() {
        let root = temp("nested");
        let parent = claude_fixture(&root);
        add_claude_agent(
            &parent,
            "outer",
            json!({ "agentType": "general-purpose", "description": "Outer", "toolUseId": "toolu_top", "spawnDepth": 1 }),
            &[side_user("go"), side_tool("a", "toolu_inner", "Agent", json!({ "description": "Inner" }))],
        );
        add_claude_agent(
            &parent,
            "inner",
            json!({ "agentType": "Explore", "description": "Inner", "toolUseId": "toolu_inner", "spawnDepth": 2 }),
            &[side_user("dig")],
        );
        let list = collect(&mut States::new(), "s", Agent::Claude, Some(&parent), &[], None, now_secs());
        let by = |id: &str| list.iter().find(|a| a.id == id).unwrap();
        assert_eq!(by("outer").parent, None);
        assert_eq!(by("inner").parent.as_deref(), Some("outer"));
        assert_eq!(by("inner").depth, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_helper_with_metadata_but_no_log_yet_is_listed_without_activity() {
        let root = temp("nolog");
        let parent = claude_fixture(&root);
        let dir = parent.with_extension("").join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent-zz.meta.json"), json!({ "agentType": "Plan", "description": "Soon", "toolUseId": "t" }).to_string()).unwrap();
        let list = collect(&mut States::new(), "s", Agent::Claude, Some(&parent), &[], None, now_secs());
        assert_eq!((list[0].has_log, list[0].running, list[0].tools), (false, false, 0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hostile_or_damaged_entries_are_skipped() {
        let root = temp("bad");
        let parent = claude_fixture(&root);
        let dir = parent.with_extension("").join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent-a b.meta.json"), "{}").unwrap(); // a space is not an id
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        add_claude_agent(&parent, "ok", json!(null), &[side_user("x")]); // unreadable meta still lists
        let list = collect(&mut States::new(), "s", Agent::Claude, Some(&parent), &[], None, now_secs());
        assert_eq!(list.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec!["ok"]);
        assert_eq!(list[0].kind, "agent");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_conversation_folder_means_no_helpers() {
        let root = temp("none");
        let parent = claude_fixture(&root);
        assert!(collect(&mut States::new(), "s", Agent::Claude, Some(&parent), &[], None, now_secs()).is_empty());
        assert!(collect(&mut States::new(), "s", Agent::Claude, None, &[], None, now_secs()).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_helpers_come_from_the_parent_log_and_their_own_rollouts() {
        let root = temp("codex");
        let day = root.join("2026/10/09");
        write_lines(
            &day.join("rollout-2026-10-09T10-00-00-th-child1.jsonl"),
            &[
                json!({ "timestamp": "2026-10-09T10:00:01Z", "type": "event_msg", "payload": { "type": "item_completed",
                    "item": { "type": "UserMessage", "id": "m1", "content": [{ "type": "input_text", "text": "search it" }] } } }),
                json!({ "timestamp": "2026-10-09T10:00:09Z", "type": "event_msg", "payload": { "type": "item_completed",
                    "item": { "type": "AgentMessage", "id": "m2", "content": [{ "type": "output_text", "text": "Done: 3 hits." }] } } }),
            ],
        );
        let mut parent = ChatState::new(Agent::Codex, "conv");
        for (kind, id, thread) in [("started", "call_1", "th-child1"), ("started", "call_2", "th-child2"), ("completed", "done-1", "th-child1")] {
            let line = json!({ "type": "event_msg", "payload": { "type": "item_completed", "item": {
                "type": "SubAgentActivity", "id": id, "kind": kind, "agent_thread_id": thread, "agent_path": format!("/root/{}", if thread.ends_with('1') { "search_facts" } else { "later_one" }) } } });
            parent.ingest(line.to_string().as_bytes());
        }
        // Messages to the main thread are not helpers.
        parent.ingest(json!({ "type": "event_msg", "payload": { "type": "item_completed", "item": {
            "type": "SubAgentActivity", "id": "x", "kind": "interacted", "agent_thread_id": "th-root", "agent_path": "/root" } } }).to_string().as_bytes());
        assert_eq!(parent.spawns().len(), 2);

        let list = collect(&mut States::new(), "s", Agent::Codex, None, parent.spawns(), Some(&root), now_secs());
        let by = |id: &str| list.iter().find(|a| a.id == id).unwrap();
        let one = by("th-child1");
        assert_eq!((one.kind.as_str(), one.description.as_str()), ("codex", "search facts"));
        assert_eq!(one.tool_use_id.as_deref(), Some("call_1"));
        assert!(!one.running, "a completed event ends it");
        assert!(one.has_log);
        assert_eq!(one.result.as_deref(), Some("Done: 3 hits."));
        let two = by("th-child2");
        assert!(two.running && !two.has_log, "started, no rollout yet");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Prints the helpers found for a real conversation log:
    /// `TTM_REAL_LOG=~/.claude/projects/<project>/<id>.jsonl cargo test real_helpers -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads a real log named by TTM_REAL_LOG"]
    fn real_helpers() {
        let Some(path) = std::env::var_os("TTM_REAL_LOG").map(PathBuf::from) else { return };
        let mut states = States::new();
        let list = collect(&mut states, "real", Agent::Claude, Some(&path), &[], None, now_secs());
        eprintln!("{} helpers", list.len());
        for a in &list {
            eprintln!(
                "{:>8} {:<16} depth={} parent={:?} tools={:<3} running={:<5} log={} | {} | last: {:?}",
                &a.id[..a.id.len().min(8)], a.kind, a.depth, a.parent.as_deref().map(|p| &p[..p.len().min(6)]), a.tools, a.running, a.has_log,
                a.description, a.last_action
            );
        }
        assert!(list.iter().all(|a| a.has_log));
    }
}
