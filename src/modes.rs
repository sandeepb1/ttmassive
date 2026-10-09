//! Modes and other toggles for the chat view: plan or build, what Codex may do, reasoning up and
//! down, and one-tap commands. Everything works by pressing the keys the agent's own TUI expects
//! and reading the result off the screen. What those keys do was found by trying them (Claude Code
//! 2.1.295, Codex 0.161.0):
//!
//! - **Claude: `Shift+Tab` cycles modes.** A normal launch cycles auto → manual → accept edits →
//!   plan → auto. A launch with `--dangerously-skip-permissions` has a fifth, `bypass permissions`.
//!   The footer always names the current mode, so the driver presses until it reads the target.
//! - **Codex: `Shift+Tab` toggles default ⇄ plan.** The footer shows `Plan mode (⇧tab to cycle)`
//!   in plan mode and nothing in default mode. `Alt+,` and `Alt+.` step the reasoning down and up.
//! - **Codex `/permissions`** lists four presets and applies the one you pick (it does not write
//!   `config.toml`). *Full Access* asks "Enable full access?" first.
//! - Keys that arrive in a burst are dropped, so every step is checked against the screen.
//!
//! Codex's `Esc Esc` is deliberately not offered. It opens a rewind browser, and a rewind leaves
//! the conversation log untouched, so a chat built from the log would keep showing turns that
//! Codex has discarded.

use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    config::Agent,
    modelmenu::{self, CODEX_PERMISSIONS, ModelMenu, codex_rows, ensure_ready, move_to, press, screen_text, type_text, wait_for},
    ops,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Codex's normal mode.
    Default,
    /// Claude's automatic approval.
    Auto,
    Manual,
    AcceptEdits,
    Plan,
    /// Claude launched with `--dangerously-skip-permissions`.
    Bypass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModeInfo {
    pub id: Mode,
    pub label: &'static str,
    pub hint: &'static str,
}

const fn info(id: Mode, label: &'static str, hint: &'static str) -> ModeInfo {
    ModeInfo { id, label, hint }
}

/// The modes `agent` can be switched between. Bypass exists only in a session started with
/// permissions skipped (or already in it).
pub fn available(agent: Agent, yolo: bool, current: Option<Mode>) -> Vec<ModeInfo> {
    match agent {
        Agent::Claude => {
            let mut v = vec![
                info(Mode::Plan, "Plan", "Plans without making changes"),
                info(Mode::AcceptEdits, "Accept edits", "Edits files without asking; still asks for commands"),
                info(Mode::Manual, "Manual", "Asks before edits and commands"),
                info(Mode::Auto, "Auto", "Claude's automatic approval mode"),
            ];
            if yolo || current == Some(Mode::Bypass) {
                v.push(info(Mode::Bypass, "Bypass permissions", "Skips every permission check"));
            }
            v
        }
        Agent::Codex => vec![
            info(Mode::Default, "Default", "Codex's normal mode"),
            info(Mode::Plan, "Plan", "Plans first; reasoning goes up while planning"),
        ],
        Agent::Other => Vec::new(),
    }
}

/// The last few non-empty lines, bottom first: where an agent keeps its status footer.
fn footer(screen: &str) -> impl Iterator<Item = &str> {
    screen.lines().rev().map(str::trim).filter(|l| !l.is_empty()).take(6)
}

/// Which mode `agent` is in, read from its footer. `None` when the footer is not showing
/// (a menu is open, or the agent is still starting).
pub fn detect(agent: Agent, screen: &str) -> Option<Mode> {
    match agent {
        Agent::Claude => footer(screen).find_map(|l| {
            let l = l.to_lowercase();
            if l.contains("bypass permissions on") {
                Some(Mode::Bypass)
            } else if l.contains("auto mode on") {
                Some(Mode::Auto)
            } else if l.contains("manual mode on") {
                Some(Mode::Manual)
            } else if l.contains("accept edits on") {
                Some(Mode::AcceptEdits)
            } else if l.contains("plan mode on") {
                Some(Mode::Plan)
            } else {
                None
            }
        }),
        Agent::Codex => {
            let lines: Vec<&str> = footer(screen).collect();
            if lines.iter().any(|l| l.contains("Plan mode (")) {
                Some(Mode::Plan)
            } else if lines.iter().any(|l| l.contains(" · ~") || l.contains("? for shortcuts")) {
                Some(Mode::Default)
            } else {
                None
            }
        }
        Agent::Other => None,
    }
}

/// Codex's reasoning level from its footer: `GPT-6.1-Sol high · ~/proj` is `high`.
pub fn codex_footer_effort(screen: &str) -> Option<String> {
    footer(screen).find_map(|l| {
        let (head, _) = l.split_once(" · ")?;
        let mut words = head.split_whitespace();
        let level = words.next_back()?;
        // Something must come before the level (the model name).
        (LEVELS.contains(&level) && words.next().is_some()).then(|| level.to_owned())
    })
}

const LEVELS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

/// Claude's reasoning level from the status above its prompt: `◐ medium · /effort`.
pub fn claude_footer_effort(screen: &str) -> Option<String> {
    screen.lines().rev().take(12).find_map(|l| {
        let (head, _) = l.trim().split_once(" · /effort")?;
        let level = head.split_whitespace().next_back()?;
        LEVELS.contains(&level).then(|| level.to_owned())
    })
}

/// The reasoning level the agent is using right now, read from its screen. More current than the
/// conversation log, which only learns it from the agent's next reply.
pub fn live_effort(agent: Agent, screen: &str) -> Option<String> {
    match agent {
        Agent::Claude => claude_footer_effort(screen),
        Agent::Codex => codex_footer_effort(screen),
        Agent::Other => None,
    }
}

// -------------------------------------------------------------------- driving

fn agent_of(st: &AppState, id: &str) -> Result<Agent> {
    st.sessions
        .get(id)
        .and_then(|m| m.agent())
        .filter(|a| *a != Agent::Other)
        .context("this session is not running Claude Code or Codex")
}

/// Put the agent in `target` mode.
pub async fn switch(st: &Arc<AppState>, id: &str, target: Mode) -> Result<Mode> {
    let agent = agent_of(st, id)?;
    ensure_ready(st, id, agent).await?;
    // A cycle is at most five modes long; one more press shows it came back round.
    for _ in 0..7 {
        let now = detect(agent, &screen_text(id).await?).context("could not read the current mode from the screen")?;
        if now == target {
            return Ok(now);
        }
        press(id, "BTab").await?;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    bail!("that mode is not available in this session")
}

/// Codex only: one step down or up in reasoning. Returns the new level.
pub async fn codex_reasoning(st: &Arc<AppState>, id: &str, up: bool) -> Result<String> {
    ensure_ready(st, id, Agent::Codex).await?;
    let before = codex_footer_effort(&screen_text(id).await?).context("could not read the reasoning level")?;
    press(id, if up { "M-." } else { "M-," }).await?;
    let changed = wait_for(id, "the reasoning level to change", |s| codex_footer_effort(s).is_some_and(|e| e != before)).await;
    match changed {
        Ok(screen) => codex_footer_effort(&screen).context("could not read the new reasoning level"),
        Err(_) => bail!("reasoning is already at its {}", if up { "highest" } else { "lowest" }),
    }
}

/// Commands that are safe to send with one tap. Each is typed into the session exactly as you
/// would. `/cost` and `/status` are left out: they open a dialog that has to be dismissed.
pub fn quick_command(agent: Agent, name: &str) -> Option<&'static str> {
    match (agent, name) {
        (Agent::Claude, "compact") | (Agent::Codex, "compact") => Some("/compact"),
        (Agent::Claude, "context") => Some("/context"),
        (Agent::Claude, "clear") => Some("/clear"),
        (Agent::Codex, "new") => Some("/new"),
        _ => None,
    }
}

pub async fn run_quick_command(st: &Arc<AppState>, id: &str, name: &str) -> Result<()> {
    let agent = agent_of(st, id)?;
    let text = quick_command(agent, name).with_context(|| format!("{name:?} is not a quick command for this agent"))?;
    ops::send_message(id, text).await
}

fn perms_cursor(screen: &str) -> Option<usize> {
    codex_rows(screen, CODEX_PERMISSIONS).and_then(|(_, _, c)| c)
}

/// Codex only: open `/permissions` and read the presets. The menu stays open until a choice or
/// a close.
pub async fn perms_open(st: &Arc<AppState>, id: &str) -> Result<ModelMenu> {
    ensure_ready(st, id, Agent::Codex).await?;
    type_text(id, "/permissions").await?;
    press(id, "Enter").await?;
    let screen = wait_for(id, "the permissions menu", |s| codex_rows(s, CODEX_PERMISSIONS).is_some()).await?;
    let (_, rows, cursor) = codex_rows(&screen, CODEX_PERMISSIONS).context("could not read the permissions")?;
    Ok(ModelMenu { rows, cursor, effort: None })
}

/// With the permissions menu open, apply the preset on row `number`. Full Access asks Codex's own
/// confirmation, which is only accepted when `allow_full` says you confirmed in the app.
pub async fn perms_choose(id: &str, number: usize, allow_full: bool) -> Result<String> {
    ensure!(number >= 1, "no such preset");
    let screen = screen_text(id).await?;
    let (_, rows, _) = codex_rows(&screen, CODEX_PERMISSIONS).context("the permissions menu is not open (open it again)")?;
    let label = rows.iter().find(|r| r.number == number).map(|r| r.label.clone()).context("no such preset")?;
    move_to(id, number, perms_cursor).await?;
    press(id, "Enter").await?;
    let next = wait_for(id, "the permission to be applied", |s| {
        s.contains("Permission selection requested") && !modelmenu::picker_open(s) || s.contains("Enable full access?")
    })
    .await?;
    if next.contains("Enable full access?") {
        if !allow_full {
            modelmenu::close(id).await?;
            bail!("Full Access needs your confirmation first");
        }
        // The highlight starts on "Yes, continue anyway".
        press(id, "Enter").await?;
        wait_for(id, "full access to be applied", |s| !s.contains("Enable full access?")).await?;
    }
    Ok(format!("Permissions set to {label} for this session"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERMS: &str = include_str!("testdata/codex_perms.txt");
    const FULL: &str = include_str!("testdata/codex_fullaccess.txt");

    fn claude(footer_line: &str) -> String {
        format!("❯ \n────────\n  {footer_line}\n")
    }

    #[test]
    fn claudes_mode_is_read_from_its_footer() {
        // The exact footers seen from Claude Code 2.1.295.
        let cases = [
            ("⏵⏵ auto mode on (shift+tab to cycle) · ← for agents", Mode::Auto),
            ("⏸ manual mode on · ? for shortcuts · ← for agents", Mode::Manual),
            ("⏵⏵ accept edits on (shift+tab to cycle) · ← for agents", Mode::AcceptEdits),
            ("⏸ plan mode on (shift+tab to cycle) · ← for agents", Mode::Plan),
            ("⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents", Mode::Bypass),
        ];
        for (line, want) in cases {
            assert_eq!(detect(Agent::Claude, &claude(line)), Some(want), "{line}");
        }
        assert_eq!(detect(Agent::Claude, "❯ hello\n  just some text"), None);
    }

    #[test]
    fn a_mode_named_in_the_conversation_above_is_not_the_current_one() {
        let mut screen = String::from("  ⏸ plan mode on (shift+tab to cycle) was what I said\n");
        for i in 0..10 {
            screen.push_str(&format!("  line {i}\n"));
        }
        screen.push_str("  ⏵⏵ accept edits on (shift+tab to cycle) · ← for agents\n");
        assert_eq!(detect(Agent::Claude, &screen), Some(Mode::AcceptEdits));
    }

    #[test]
    fn codex_is_in_plan_mode_only_when_its_footer_says_so() {
        let default = "› Ask Codex to do anything\n  GPT-6.1-Sol medium · ~/Documents/odev/ttmassive\n  ← for agents · ? for shortcuts\n";
        let plan = "› Ask Codex to do anything\n  GPT-6.1-Sol high · ~/Documents/odev/ttmassive                      Plan mode (⇧tab to cycle)\n  ← for agents · ? for shortcuts\n";
        assert_eq!(detect(Agent::Codex, default), Some(Mode::Default));
        assert_eq!(detect(Agent::Codex, plan), Some(Mode::Plan));
        // A menu on screen means the footer is not showing.
        assert_eq!(detect(Agent::Codex, PERMS), None);
        assert_eq!(detect(Agent::Other, default), None);
    }

    #[test]
    fn the_modes_on_offer_depend_on_the_agent_and_how_it_was_launched() {
        let ids = |v: Vec<ModeInfo>| v.into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(
            ids(available(Agent::Claude, false, Some(Mode::Plan))),
            [Mode::Plan, Mode::AcceptEdits, Mode::Manual, Mode::Auto]
        );
        assert_eq!(ids(available(Agent::Claude, true, None)).last(), Some(&Mode::Bypass));
        // Already in bypass (e.g. started by hand) shows it even if ttm did not launch it so.
        assert!(ids(available(Agent::Claude, false, Some(Mode::Bypass))).contains(&Mode::Bypass));
        assert_eq!(ids(available(Agent::Codex, false, None)), [Mode::Default, Mode::Plan]);
        assert!(available(Agent::Other, true, None).is_empty());
    }

    #[test]
    fn codex_reasoning_is_read_from_the_footer() {
        assert_eq!(codex_footer_effort("  GPT-6.1-Sol xhigh · ~/proj").as_deref(), Some("xhigh"));
        assert_eq!(codex_footer_effort("  GPT-6-Luna low · ~/proj · Reply hi").as_deref(), Some("low"));
        assert_eq!(codex_footer_effort("  ← for agents · ? for shortcuts"), None);
        assert_eq!(codex_footer_effort("  high · ~/proj"), None, "a level with no model before it is not the footer");
    }

    #[test]
    fn claudes_reasoning_is_read_from_the_status_above_its_prompt() {
        let screen = |status: &str| format!("  ⎿  output
{status}
──────
❯ 
──────
  ⏸ plan mode on (shift+tab to cycle)
");
        let pad = " ".repeat(80);
        assert_eq!(claude_footer_effort(&screen(&format!("{pad}◐ medium · /effort"))).as_deref(), Some("medium"));
        assert_eq!(claude_footer_effort(&screen(&format!("{pad}● high · /effort"))).as_deref(), Some("high"));
        assert_eq!(claude_footer_effort(&screen("no status here")), None);
        assert_eq!(claude_footer_effort(&screen("◐ turbo · /effort")), None, "only known levels count");
        assert_eq!(live_effort(Agent::Codex, "  GPT-6.1-Sol low · ~/proj").as_deref(), Some("low"));
        assert_eq!(live_effort(Agent::Other, "◐ medium · /effort"), None);
    }

    #[test]
    fn quick_commands_are_an_allow_list_per_agent() {
        assert_eq!(quick_command(Agent::Claude, "compact"), Some("/compact"));
        assert_eq!(quick_command(Agent::Claude, "clear"), Some("/clear"));
        assert_eq!(quick_command(Agent::Codex, "new"), Some("/new"));
        assert_eq!(quick_command(Agent::Codex, "clear"), None);
        assert_eq!(quick_command(Agent::Claude, "new"), None);
        // Nothing that opens a dialog, and nothing that is not listed.
        assert_eq!(quick_command(Agent::Claude, "cost"), None);
        assert_eq!(quick_command(Agent::Codex, "status"), None);
        assert_eq!(quick_command(Agent::Claude, "compact; rm -rf ~"), None);
        assert_eq!(quick_command(Agent::Other, "compact"), None);
    }

    #[test]
    fn codex_permissions_are_read_with_the_current_one_and_the_cursor() {
        let (_, rows, cursor) = codex_rows(PERMS, CODEX_PERMISSIONS).unwrap();
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["Ask for approval", "Approve for me", "Full Access", "Read Only"]);
        assert_eq!(rows.iter().find(|r| r.current).map(|r| r.number), Some(4));
        assert_eq!(cursor, Some(4));
        // Wrapped description lines are not rows of their own.
        assert_eq!(rows.len(), 4);
        assert!(codex_rows(FULL, CODEX_PERMISSIONS).is_none());
    }

    #[test]
    fn the_permission_screens_count_as_pickers_so_digits_are_refused() {
        assert!(modelmenu::picker_open(PERMS));
        assert!(modelmenu::picker_open(FULL), "the Full Access confirmation must not take generic answers");
        assert_eq!(perms_cursor(PERMS), Some(4));
    }
}
