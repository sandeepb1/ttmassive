//! Operations shared by the HTTP API and the CLI control socket.

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use crate::{
    AppState,
    screen::{self, Analysis, Step},
    scrollback, sessions, tmux,
};

const MAX_TEXT: usize = 4096;
/// How long to wait between typing a message and pressing Enter.
const SUBMIT_DELAY: std::time::Duration = std::time::Duration::from_millis(350);
/// A chat message can be longer than a quick `ttm send`: people paste logs and code.
const MAX_MESSAGE: usize = 16 * 1024;
const MAX_KEYS: usize = 32;
const MAX_NOTE: usize = 500;
const MAX_TAGS: usize = 8;

/// Plain-text screen contents: live from tmux, or the saved copy for a stopped session.
pub async fn output(st: &AppState, id: &str, lines: usize) -> Result<String> {
    let lines = lines.clamp(1, scrollback::SNAPSHOT_LINES);
    if tmux::exists(id).await {
        let text = tmux::capture(id, lines).await?;
        let all: Vec<&str> = text.trim_end().lines().collect();
        return Ok(all[all.len().saturating_sub(lines)..].join("\n"));
    }
    let uid = st.sessions.get(id).map(|m| m.uid.clone()).with_context(|| format!("no such session: {id}"))?;
    scrollback::read_tail(st, &uid, lines).await.with_context(|| format!("{id} is stopped and has no saved output"))
}

/// What the session is doing and any question it is asking (live sessions only).
pub async fn screen(id: &str) -> Result<Analysis> {
    Ok(screen::analyze(&tmux::capture(id, 60).await?))
}

/// Type into a session without attaching.
pub async fn send(id: &str, text: Option<&str>, keys: &[String], enter: bool) -> Result<()> {
    ensure!(keys.len() <= MAX_KEYS, "too many keys");
    let mut steps: Vec<Step> = Vec::new();
    if let Some(t) = text.filter(|t| !t.is_empty()) {
        ensure!(t.len() <= MAX_TEXT, "text too long (max {MAX_TEXT} bytes)");
        steps.push(Step::Text(t.to_owned()));
    }
    for k in keys {
        steps.push(Step::Key(tmux::key_name(k).with_context(|| format!("unknown key {k:?}"))?));
    }
    if enter {
        steps.push(Step::Key("Enter"));
    }
    ensure!(!steps.is_empty(), "nothing to send");
    ensure!(tmux::exists(id).await, "no such running session: {id}");
    tmux::send_steps(id, &steps).await
}

/// Type a chat message into the agent and submit it. Multi-line text is pasted so its newlines
/// don't submit it early. Refuses while a question is on screen, which would swallow the text.
pub async fn send_message(id: &str, text: &str) -> Result<()> {
    let text = text.trim();
    ensure!(!text.is_empty(), "nothing to send");
    ensure!(text.len() <= MAX_MESSAGE, "message too long (max {MAX_MESSAGE} bytes)");
    ensure!(tmux::exists(id).await, "no such running session: {id}");
    ensure!(screen(id).await?.prompt.is_none(), "the agent is asking a question: answer it first");
    if text.contains('\n') {
        tmux::paste(id, text).await?;
    } else {
        tmux::send_steps(id, &[Step::Text(text.to_owned())]).await?;
    }
    // Pause before the Enter that submits. Codex reads text that arrives in one burst as a paste
    // and turns an Enter right behind it into a new line, so the message would sit unsent.
    tokio::time::sleep(SUBMIT_DELAY).await;
    tmux::send_steps(id, &[Step::Key("Enter")]).await
}

/// Answer the question on screen: `approve`, `deny`, `interrupt`, or a choice number.
/// When `prompt_id` is given it must still match, so a stale button can't answer a new question.
pub async fn answer(id: &str, choice: &str, prompt_id: Option<&str>) -> Result<String> {
    ensure!(tmux::exists(id).await, "no such running session: {id}");
    if choice == "interrupt" {
        tmux::send_steps(id, &[Step::Key("Escape")]).await?;
        return Ok("interrupt".into());
    }
    ensure!(
        !crate::modelmenu::picker_open(&tmux::capture(id, 80).await?),
        "the model or effort menu is open: use the model button in the chat, or press Esc"
    );
    let prompt = screen(id).await?.prompt.context("no question is showing on that screen")?;
    if let Some(want) = prompt_id {
        ensure!(prompt.id == want, "the question changed; check the screen and try again");
    }
    let index = match choice {
        "approve" | "yes" => screen::approve_index(&prompt),
        "deny" | "no" => screen::deny_index(&prompt),
        n => n.parse::<usize>().with_context(|| format!("unknown choice {n:?}"))?,
    };
    let steps = screen::steps_for(&prompt, index).with_context(|| format!("no choice number {index}"))?;
    tmux::send_steps(id, &steps).await?;
    let label = prompt.choices.iter().find(|c| c.index == index).map(|c| c.label.clone()).unwrap_or_default();
    Ok(label)
}

#[derive(Debug, Default, Deserialize)]
pub struct MetaPatch {
    /// Empty string clears the note.
    pub note: Option<String>,
    pub tags: Option<Vec<String>>,
    pub pinned: Option<bool>,
}

fn valid_tag(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 24
        && t.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

pub async fn set_meta(st: &AppState, id: &str, patch: MetaPatch) -> Result<()> {
    if let Some(n) = &patch.note {
        ensure!(n.chars().count() <= MAX_NOTE, "note too long (max {MAX_NOTE} characters)");
    }
    if let Some(t) = &patch.tags {
        ensure!(t.len() <= MAX_TAGS, "too many tags (max {MAX_TAGS})");
        ensure!(t.iter().all(|x| valid_tag(x)), "tags are 1-24 chars of letters, digits, - _ .");
    }
    {
        let mut m = st.sessions.get_mut(id).with_context(|| format!("no such session: {id}"))?;
        if let Some(n) = patch.note {
            m.note = (!n.trim().is_empty()).then(|| n.trim().to_owned());
        }
        if let Some(mut t) = patch.tags {
            t.sort();
            t.dedup();
            m.tags = t;
        }
        if let Some(p) = patch.pinned {
            m.pinned = p;
        }
    }
    sessions::save(st).await;
    Ok(())
}

/// Start every preset in a configured group; returns the new session names.
pub async fn start_group(st: &AppState, name: &str) -> Result<Vec<String>> {
    let group = st.cfg.groups.iter().find(|g| g.name == name).with_context(|| format!("unknown group {name:?}"))?;
    let mut ids = Vec::new();
    for preset in &group.presets {
        let id = sessions::create(
            st,
            sessions::NewSession { preset: Some(preset), name: None, group: Some(name), exact: false, yolo: false, dir: None },
        )
        .await?;
        ids.push(id);
    }
    Ok(ids)
}

/// A hook reported the agent's own conversation id; remember it for resuming.
pub async fn record_agent_session(st: &AppState, uid: &str, agent_session_id: Option<&str>, cwd: Option<&str>) {
    let Some(id) = sessions::name_for_uid(st, uid) else { return };
    // Codex runs a background thread to name each conversation, and that thread fires the same
    // hook with its own id. Only an id that has a conversation log is the real one.
    let mut agent_session_id = agent_session_id.filter(|a| sessions::valid_agent_id(a));
    if let Some(a) = agent_session_id
        && st.sessions.get(&id).and_then(|m| m.agent()) == Some(crate::config::Agent::Codex)
    {
        let thread = a.to_owned();
        let has_log = tokio::task::spawn_blocking(move || crate::agentlog::find_log(crate::config::Agent::Codex, &thread).is_some())
            .await
            .unwrap_or(false);
        if !has_log {
            tracing::debug!("ignoring a Codex hook for thread {a}: it has no conversation log");
            agent_session_id = None;
        }
    }
    let mut changed = false;
    if let Some(mut m) = st.sessions.get_mut(&id) {
        if let Some(a) = agent_session_id
            && m.agent_session_id.as_deref() != Some(a)
        {
            m.agent_session_id = Some(a.to_owned());
            changed = true;
        }
        if m.spec.dir.is_none()
            && let Some(c) = cwd.filter(|c| c.starts_with('/'))
        {
            m.spec.dir = Some(c.into());
            changed = true;
        }
    }
    if changed {
        sessions::save(st).await;
    }
}
