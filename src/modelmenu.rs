//! Switching Claude Code's model and effort from the chat view, by driving its own `/model` and
//! `/effort` menus. What those menus do was found by trying them (Claude Code 2.1.295):
//!
//! - `/model` is a numbered list that scrolls and shows about ten rows. `Home` jumps to the first
//!   row and `End` to the last, so the two views together are the whole list.
//! - **Pressing a row's number, or Enter, makes that model the default for every new session.**
//!   ttm never does that. It moves the cursor to the row with the arrow keys, then presses `s`
//!   ("use this session only"), then answers the "Switch model?" confirmation.
//! - `/effort` is a slider: `←`/`→` move it, `s` applies it to this session only.
//! - Keys that arrive in a burst are dropped, so every move is checked against the screen.
//!
//! Codex has its own `/model` menu, and it works the same way. Checked with a throwaway
//! `CODEX_HOME` (Codex 0.161.0): it is two screens, the model and then its reasoning level.
//! **Enter on the level screen rewrites `model` and `model_reasoning_effort` in `config.toml`**,
//! the default for every new session. `s` on that screen ("session") changes this session only
//! and writes nothing. ttm only ever presses `s` there. Rows are marked `(current)` and
//! `(default)`, the highlight is `›`, and the cursor wraps around at both ends.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use std::collections::HashSet;

use crate::{AppState, config::Agent, ops, screen::Activity, screen::Step, tmux};

const KEY_GAP: Duration = Duration::from_millis(170);
const POLL_EVERY: Duration = Duration::from_millis(150);
const WAIT: Duration = Duration::from_secs(4);
/// More moves than any list has rows; a guard against looping if a key never lands.
const MAX_MOVES: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelRow {
    pub number: usize,
    pub label: String,
    pub description: String,
    /// The model this session is using now.
    pub current: bool,
    /// Codex marks the model or level a new session starts with.
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelMenu {
    pub rows: Vec<ModelRow>,
    /// The row the highlight is on.
    #[serde(skip)]
    pub cursor: Option<usize>,
    /// The effort line under the list, such as `Medium effort (default)`.
    pub effort: Option<String>,
}

/// Where the model menu starts on a screen: the line that says `Select model`.
fn menu_start(screen: &str) -> Option<usize> {
    screen.lines().position(|l| l.trim() == "Select model")
}

/// `(on the cursor, number, label, description)` for a menu row such as
/// `  ❯ 14. Opus Plan Mode ✔       Use Opus in plan mode`.
fn parse_row(line: &str) -> Option<(bool, usize, String, String, bool)> {
    let t = line.trim_start();
    let first = t.chars().next()?;
    let (cursor, rest) = match first {
        '❯' => (true, &t[first.len_utf8()..]),
        '↑' | '↓' => (false, &t[first.len_utf8()..]),
        _ => (false, t),
    };
    let rest = rest.trim_start();
    let digits = rest.find(|c: char| !c.is_ascii_digit())?;
    if digits == 0 {
        return None;
    }
    let number: usize = rest[..digits].parse().ok()?;
    let body = rest[digits..].strip_prefix('.')?.trim();
    if body.is_empty() {
        return None;
    }
    // The label and its description are separated by two or more spaces.
    let (label, description) = match body.find("  ") {
        Some(i) => (body[..i].trim(), body[i..].trim()),
        None => (body, ""),
    };
    let current = label.ends_with('✔');
    let label = label.trim_end_matches('✔').trim();
    Some((cursor, number, label.to_owned(), description.to_owned(), current))
}

/// `(on the cursor, number, label, description, current, default)` for a Codex row such as
/// `› 2. GPT-6-Astra (current)  Frontier intelligence`.
fn parse_codex_row(line: &str) -> Option<(bool, usize, String, String, bool, bool)> {
    let t = line.trim_start();
    let on = t.starts_with('›');
    let t = t.strip_prefix('›').unwrap_or(t);
    let (_, number, label, description, _) = parse_row(t)?;
    let current = label.contains("(current)");
    let default = label.contains("(default)");
    let label = label.replace("(current)", "").replace("(default)", "");
    Some((on, number, label.trim().to_owned(), description, current, default))
}

/// The model list on `screen`, if the menu is showing.
pub fn parse_menu(screen: &str) -> Option<ModelMenu> {
    let start = menu_start(screen)?;
    let mut rows = Vec::new();
    let mut cursor = None;
    let mut effort = None;
    for line in screen.lines().skip(start + 1) {
        if let Some((on, number, label, description, current)) = parse_row(line) {
            if on {
                cursor = Some(number);
            }
            rows.push(ModelRow { number, label, description, current, default: false });
        } else if line.contains("←/→ to adjust") {
            let text = line.trim_start().trim_start_matches(|c: char| !c.is_alphanumeric());
            effort = text.split("←/→").next().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        }
    }
    (!rows.is_empty()).then_some(ModelMenu { rows, cursor, effort })
}

/// Rows from two views of the list, combined, in order.
pub fn merge_rows(a: Vec<ModelRow>, b: Vec<ModelRow>) -> Vec<ModelRow> {
    let mut all = a;
    for r in b {
        if !all.iter().any(|x| x.number == r.number) {
            all.push(r);
        }
    }
    all.sort_by_key(|r| r.number);
    all
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortSlider {
    pub levels: Vec<String>,
    /// Index into `levels` of the one the marker is under.
    pub current: usize,
}

/// The effort slider on `screen`: the levels along its bottom edge and which one the `▲` is over.
pub fn parse_effort(screen: &str) -> Option<EffortSlider> {
    let lines: Vec<&str> = screen.lines().collect();
    let marker_line = lines.iter().position(|l| l.contains('▲'))?;
    let marker = lines[marker_line].chars().position(|c| c == '▲')?;
    let words_line = lines.get(marker_line + 1)?;
    // Each word and the column it starts at.
    let mut words: Vec<(usize, String)> = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (col, ch) in words_line.chars().enumerate() {
        if ch.is_whitespace() {
            words.extend(current.take());
        } else {
            current.get_or_insert((col, String::new())).1.push(ch);
        }
    }
    words.extend(current);
    if words.len() < 2 {
        return None;
    }
    let centre = |(start, w): &(usize, String)| *start as f64 + w.chars().count() as f64 / 2.0;
    let current = (0..words.len())
        .min_by(|&a, &b| {
            let da = (centre(&words[a]) - marker as f64).abs();
            let db = (centre(&words[b]) - marker as f64).abs();
            da.total_cmp(&db)
        })
        .unwrap_or(0);
    Some(EffortSlider { levels: words.into_iter().map(|(_, w)| w).collect(), current })
}

const CODEX_MODELS: &str = "Select Model and Effort";
/// Codex's `/permissions` screen: what it may do without asking.
pub(crate) const CODEX_PERMISSIONS: &str = "Update Model Permissions";
const CODEX_LEVELS: &str = "Select Reasoning Level for ";

/// The rows under the line that starts with `header`, and which one the highlight is on.
pub(crate) fn codex_rows(screen: &str, header: &str) -> Option<(String, Vec<ModelRow>, Option<usize>)> {
    let lines: Vec<&str> = screen.lines().collect();
    let at = lines.iter().position(|l| l.trim_start().starts_with(header))?;
    let title = lines[at].trim().to_owned();
    let mut rows = Vec::new();
    let mut cursor = None;
    for line in &lines[at + 1..] {
        if let Some((on, number, label, description, current, default)) = parse_codex_row(line) {
            if on {
                cursor = Some(number);
            }
            rows.push(ModelRow { number, label, description, current, default });
        }
    }
    (!rows.is_empty()).then_some((title, rows, cursor))
}

/// Codex's model list.
pub fn parse_codex_models(screen: &str) -> Option<ModelMenu> {
    let (_, rows, cursor) = codex_rows(screen, CODEX_MODELS)?;
    Some(ModelMenu { rows, cursor, effort: None })
}

/// Codex's reasoning-level screen for the model just picked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LevelMenu {
    pub model: String,
    pub rows: Vec<ModelRow>,
    #[serde(skip)]
    pub cursor: Option<usize>,
}

pub fn parse_codex_levels(screen: &str) -> Option<LevelMenu> {
    let (title, rows, cursor) = codex_rows(screen, CODEX_LEVELS)?;
    let model = title.strip_prefix(CODEX_LEVELS.trim_end())?.trim().to_owned();
    Some(LevelMenu { model, rows, cursor })
}

/// The row of a level screen that means `level` (`low`, `medium`, `high`, `xhigh`).
pub fn level_row(menu: &LevelMenu, level: &str) -> Option<usize> {
    let want = match level.to_ascii_lowercase().as_str() {
        "xhigh" | "extra high" | "extra-high" => "extra high".to_owned(),
        other => other.to_owned(),
    };
    menu.rows.iter().find(|r| r.label.to_ascii_lowercase() == want).map(|r| r.number)
}

// -------------------------------------------------------------------- driving

pub(crate) async fn press(id: &str, key: &'static str) -> Result<()> {
    tmux::send_steps(id, &[Step::Key(key)]).await?;
    tokio::time::sleep(KEY_GAP).await;
    Ok(())
}

pub(crate) async fn type_text(id: &str, text: &str) -> Result<()> {
    tmux::send_steps(id, &[Step::Text(text.to_owned())]).await?;
    tokio::time::sleep(KEY_GAP).await;
    Ok(())
}

pub(crate) async fn screen_text(id: &str) -> Result<String> {
    tmux::capture(id, 80).await
}

/// Poll the screen until `done` holds for it, and return it.
pub(crate) async fn wait_for(id: &str, what: &str, done: impl Fn(&str) -> bool) -> Result<String> {
    let started = tokio::time::Instant::now();
    loop {
        let screen = screen_text(id).await?;
        if done(&screen) {
            return Ok(screen);
        }
        ensure!(started.elapsed() < WAIT, "timed out waiting for {what}");
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// Is the model list or the effort slider showing? Pressing a number or Enter there changes the
/// default for every new session, so the generic answer buttons must not drive it.
pub fn picker_open(screen: &str) -> bool {
    menu_start(screen).is_some()
        || screen.contains("to adjust · Enter to confirm")
        || parse_codex_models(screen).is_some()
        || parse_codex_levels(screen).is_some()
        || codex_rows(screen, CODEX_PERMISSIONS).is_some()
        || screen.contains("Enable full access?")
        || crate::resume::resume_open(screen)
}

/// Is a model or effort menu (or its confirmation) showing?
fn menu_showing(screen: &str) -> bool {
    picker_open(screen) || screen.contains("Switch model?")
}

/// The agent must be at its prompt, not working and not asking something.
pub(crate) async fn ensure_ready(st: &Arc<AppState>, id: &str, want: Agent) -> Result<()> {
    let agent = st.sessions.get(id).and_then(|m| m.agent());
    ensure!(agent == Some(want), "this session is not running {}", if want == Agent::Claude { "Claude Code" } else { "Codex" });
    ensure!(tmux::exists(id).await, "no such running session: {id}");
    let a = ops::screen(id).await?;
    ensure!(a.activity != Activity::Working, "wait for the agent to finish what it is doing first");
    ensure!(a.prompt.is_none(), "the agent is asking a question: answer it first");
    Ok(())
}

/// Open `/model` and read the whole list.
pub async fn open(st: &Arc<AppState>, id: &str) -> Result<ModelMenu> {
    ensure_ready(st, id, Agent::Claude).await?;
    type_text(id, "/model").await?;
    press(id, "Enter").await?;
    wait_for(id, "the model menu", |s| menu_start(s).is_some()).await?;
    // How long is the list? `End` puts the highlight on the last row.
    press(id, "End").await?;
    let bottom = parse_menu(&screen_text(id).await?).context("could not read the end of the model list")?;
    let last = bottom.cursor.context("could not find the last model")?;
    // The menu shows only as many rows as the terminal is tall. Walk down from the top: every row
    // is on screen at least once while the highlight is on it.
    press(id, "Home").await?;
    let first = parse_menu(&screen_text(id).await?).context("could not read the model list")?;
    let effort = first.effort.clone();
    let mut rows = merge_rows(first.rows, bottom.rows);
    let mut at = first.cursor;
    for _ in 0..MAX_MOVES {
        if at.is_none_or(|n| n >= last) {
            break;
        }
        press(id, "Down").await?;
        let seen = parse_menu(&screen_text(id).await?).context("lost the model menu")?;
        at = seen.cursor;
        rows = merge_rows(rows, seen.rows);
    }
    press(id, "Home").await?; // leave the highlight on row 1: a known place to count from
    Ok(ModelMenu { rows, cursor: Some(1), effort })
}

/// Close whatever model or effort menu is open, one Escape at a time (a stray Escape at the
/// prompt can open Claude's rewind menu).
pub async fn close(id: &str) -> Result<()> {
    for _ in 0..4 {
        if !menu_showing(&screen_text(id).await?) {
            return Ok(());
        }
        press(id, "Escape").await?;
        tokio::time::sleep(KEY_GAP).await;
    }
    Ok(())
}

/// With the model menu open, move the highlight to row `number`, then `s`, then confirm.
/// Returns what Claude said, such as `Set model to Sonnet 5 for this session only`.
pub async fn choose(id: &str, number: usize) -> Result<String> {
    ensure!(number >= 1, "no such model");
    parse_menu(&screen_text(id).await?).context("the model menu is not open (open it again)")?;
    // Count from the top whatever the highlight was on, then step to the row, checking each move.
    press(id, "Home").await?;
    for _ in 0..MAX_MOVES {
        let at = parse_menu(&screen_text(id).await?).and_then(|m| m.cursor).context("lost the model menu")?;
        if at == number {
            break;
        }
        press(id, if at < number { "Down" } else { "Up" }).await?;
    }
    let at = parse_menu(&screen_text(id).await?).and_then(|m| m.cursor);
    ensure!(at == Some(number), "could not move to that model");
    type_text(id, "s").await?;
    // Switching mid-conversation asks for confirmation; a fresh one may not.
    let after = wait_for(id, "the switch to finish", |s| {
        s.contains("Switch model?") || s.contains("Set model to") || s.contains("Kept model")
    })
    .await?;
    if after.contains("Switch model?") {
        type_text(id, "1").await?;
        let done = wait_for(id, "the switch to finish", |s| s.contains("Set model to") || s.contains("Kept model")).await?;
        return Ok(result_line(&done));
    }
    Ok(result_line(&after))
}

/// The last line saying what happened.
fn result_line(screen: &str) -> String {
    screen
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.contains("Set model to") || l.contains("Kept model"))
        .map(|l| l.trim_start_matches(|c: char| !c.is_alphanumeric()).to_owned())
        .unwrap_or_else(|| "Model changed".into())
}

/// Set this session's effort to `level` through the `/effort` slider.
pub async fn set_effort(st: &Arc<AppState>, id: &str, level: &str) -> Result<String> {
    close(id).await?; // a model menu left open would swallow the next command
    ensure_ready(st, id, Agent::Claude).await?;
    type_text(id, "/effort").await?;
    press(id, "Enter").await?;
    let screen = wait_for(id, "the effort slider", |s| parse_effort(s).is_some()).await?;
    let slider = parse_effort(&screen).context("could not read the effort slider")?;
    let want = slider
        .levels
        .iter()
        .position(|l| l.eq_ignore_ascii_case(level))
        .with_context(|| format!("this model has no {level:?} effort (it offers {})", slider.levels.join(", ")))?;
    for _ in 0..MAX_MOVES {
        let at = parse_effort(&screen_text(id).await?).map(|s| s.current).context("lost the effort slider")?;
        if at == want {
            break;
        }
        press(id, if at < want { "Right" } else { "Left" }).await?;
    }
    let at = parse_effort(&screen_text(id).await?).map(|s| s.current);
    if at != Some(want) {
        close(id).await?;
        bail!("could not move the effort slider");
    }
    type_text(id, "s").await?;
    wait_for(id, "the effort to be set", |s| !s.contains("to adjust · Enter to confirm")).await?;
    Ok(format!("Effort set to {}", slider.levels[want]))
}

/// Move the highlight to row `target` of the menu `read` understands, checking after every key.
/// The list wraps around, so the shorter way is not assumed: it just steps until it arrives.
pub(crate) async fn move_to(id: &str, target: usize, read: impl Fn(&str) -> Option<usize>) -> Result<()> {
    for _ in 0..MAX_MOVES {
        let at = read(&screen_text(id).await?).context("lost the menu")?;
        if at == target {
            return Ok(());
        }
        press(id, if at < target { "Down" } else { "Up" }).await?;
    }
    bail!("could not move to that row")
}

fn models_cursor(s: &str) -> Option<usize> {
    parse_codex_models(s).and_then(|m| m.cursor)
}

fn levels_cursor(s: &str) -> Option<usize> {
    parse_codex_levels(s).and_then(|m| m.cursor)
}

/// Open Codex's `/model` menu and read its list. It stays open until a choice or a close.
pub async fn codex_open(st: &Arc<AppState>, id: &str) -> Result<ModelMenu> {
    ensure_ready(st, id, Agent::Codex).await?;
    type_text(id, "/model").await?;
    press(id, "Enter").await?;
    let first = wait_for(id, "the model menu", |s| parse_codex_models(s).is_some()).await?;
    let first = parse_codex_models(&first).context("could not read the model list")?;
    let mut rows = first.rows;
    let mut seen_at: HashSet<usize> = first.cursor.into_iter().collect();
    // The highlight starts on the current model and wraps. Step round once: every row is on
    // screen while the highlight is on it, however small the terminal.
    for _ in 0..MAX_MOVES {
        press(id, "Down").await?;
        let m = parse_codex_models(&screen_text(id).await?).context("lost the model menu")?;
        rows = merge_rows(rows, m.rows);
        match m.cursor {
            Some(c) if seen_at.insert(c) => {}
            _ => break, // back where it started
        }
    }
    Ok(ModelMenu { rows, cursor: None, effort: None })
}

/// With the model menu open, pick the model on row `number`. Codex then asks for a reasoning
/// level: this returns that screen and leaves it open.
pub async fn codex_choose(id: &str, number: usize) -> Result<LevelMenu> {
    ensure!(number >= 1, "no such model");
    ensure!(parse_codex_models(&screen_text(id).await?).is_some(), "the model menu is not open (open it again)");
    move_to(id, number, models_cursor).await?;
    press(id, "Enter").await?; // opens the level screen: nothing is applied yet
    let screen = wait_for(id, "the reasoning levels", |s| parse_codex_levels(s).is_some()).await?;
    parse_codex_levels(&screen).context("could not read the reasoning levels")
}

/// What Codex said about the change: `Model changed to gpt-6-astra medium for this session only`.
fn codex_result(screen: &str) -> String {
    screen
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.contains("Model changed to"))
        .map(|l| l.trim_start_matches(|c: char| !c.is_alphanumeric()).to_owned())
        .unwrap_or_else(|| "Model changed".into())
}

/// On the level screen, pick row `number` for this session only. Never Enter: that would make it
/// the default for every new session.
pub async fn codex_level(id: &str, number: usize) -> Result<String> {
    ensure!(number >= 1, "no such level");
    ensure!(parse_codex_levels(&screen_text(id).await?).is_some(), "the reasoning screen is not open");
    move_to(id, number, levels_cursor).await?;
    type_text(id, "s").await?;
    let done = wait_for(id, "the change to be applied", |s| s.contains("Model changed to")).await?;
    Ok(codex_result(&done))
}

/// Change Codex's reasoning effort for this session: pick the current model again, then a level.
pub async fn codex_effort(st: &Arc<AppState>, id: &str, level: &str) -> Result<String> {
    close(id).await?;
    ensure_ready(st, id, Agent::Codex).await?;
    type_text(id, "/model").await?;
    press(id, "Enter").await?;
    wait_for(id, "the model menu", |s| parse_codex_models(s).is_some()).await?;
    press(id, "Enter").await?; // the highlight starts on the current model
    let screen = wait_for(id, "the reasoning levels", |s| parse_codex_levels(s).is_some()).await?;
    let levels = parse_codex_levels(&screen).context("could not read the reasoning levels")?;
    let Some(row) = level_row(&levels, level) else {
        close(id).await?;
        let offered: Vec<String> = levels.rows.iter().map(|r| r.label.to_ascii_lowercase()).collect();
        bail!("{} has no {level:?} level (it offers {})", levels.model, offered.join(", "));
    };
    codex_level(id, row).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const CX_MODELS: &str = include_str!("testdata/codex_model.txt");
    const CX_LEVELS: &str = include_str!("testdata/codex_level.txt");
    const HOME: &str = include_str!("testdata/model_home.txt");
    const END: &str = include_str!("testdata/model_end.txt");
    const EFFORT: &str = include_str!("testdata/effort.txt");

    #[test]
    fn the_top_of_the_list_is_read_with_its_cursor_and_effort() {
        let m = parse_menu(HOME).unwrap();
        assert_eq!(m.rows.len(), 10);
        assert_eq!(m.cursor, Some(1));
        assert_eq!(m.effort.as_deref(), Some("Medium effort (default)"));
        let first = &m.rows[0];
        assert_eq!((first.number, first.label.as_str()), (1, "Default (recommended)"));
        assert_eq!(first.description, "Opus 5.5 · Best for everyday, complex tasks");
        assert!(!first.current);
        assert_eq!(m.rows[9].number, 10); // the row marked with a down arrow
        assert_eq!(m.rows[9].label, "Opus 4.8");
    }

    #[test]
    fn the_end_of_the_list_marks_the_current_model() {
        let m = parse_menu(END).unwrap();
        let current: Vec<_> = m.rows.iter().filter(|r| r.current).collect();
        assert_eq!(current.len(), 1);
        assert_eq!((current[0].number, current[0].label.as_str()), (14, "Opus Plan Mode"));
        assert_eq!(m.cursor, Some(14));
        assert_eq!(m.rows.first().map(|r| r.number), Some(5)); // scrolled: the up-arrow row
    }

    #[test]
    fn the_two_views_make_the_whole_list() {
        let all = merge_rows(parse_menu(HOME).unwrap().rows, parse_menu(END).unwrap().rows);
        let numbers: Vec<usize> = all.iter().map(|r| r.number).collect();
        assert_eq!(numbers, (1..=14).collect::<Vec<_>>());
        assert_eq!(all.iter().find(|r| r.current).map(|r| r.number), Some(14));
        // Overlapping rows are not repeated.
        assert_eq!(all.iter().filter(|r| r.number == 7).count(), 1);
    }

    #[test]
    fn a_screen_without_the_menu_is_not_a_menu() {
        assert!(parse_menu("❯ hello\n  1. not a menu").is_none());
        assert!(parse_menu("").is_none());
        // Numbered lines elsewhere on the screen are ignored: only rows after the header count.
        let screen = format!("  1. an earlier list\n  2. another\n{HOME}");
        assert_eq!(parse_menu(&screen).unwrap().rows.len(), 10);
    }

    #[test]
    fn menu_rows_are_split_into_label_and_description() {
        let (cursor, n, label, desc, current) = parse_row("   ❯ 14. Opus Plan Mode ✔       Use Opus in plan mode, Sonnet otherwise").unwrap();
        assert_eq!((cursor, n, label.as_str(), current), (true, 14, "Opus Plan Mode", true));
        assert_eq!(desc, "Use Opus in plan mode, Sonnet otherwise");
        let (cursor, n, label, desc, _) = parse_row("     3.  Sonnet 5.5             Most efficient").unwrap();
        assert_eq!((cursor, n, label.as_str(), desc.as_str()), (false, 3, "Sonnet 5.5", "Most efficient"));
        assert!(parse_row("   names, specify with --model.").is_none());
        assert!(parse_row("   10.").is_none());
    }

    #[test]
    fn the_effort_slider_is_read() {
        let s = parse_effort(EFFORT).unwrap();
        assert_eq!(s.levels, ["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(s.current, 1);
    }

    #[test]
    fn the_marker_picks_the_nearest_level_wherever_it_is() {
        let at = |col: usize| {
            let marker = format!("{}▲", "─".repeat(col));
            parse_effort(&format!("{marker}\nlow     medium     high\n")).map(|s| s.current)
        };
        assert_eq!(at(1), Some(0));
        assert_eq!(at(10), Some(1));
        assert_eq!(at(25), Some(2));
        assert_eq!(parse_effort("no slider here"), None);
        assert_eq!(parse_effort("──▲──\nonlyone\n"), None);
    }

    #[test]
    fn the_pickers_are_recognised_but_the_confirmation_is_not_one() {
        assert!(picker_open(HOME) && picker_open(END) && picker_open(EFFORT));
        assert!(!picker_open("  Switch model?\n  ❯ 1. Yes, switch to Sonnet 5\n    2. No, go back"));
        assert!(!picker_open("❯ hello\n  1. not a picker"));
    }

    #[test]
    fn menus_in_progress_are_recognised_so_they_can_be_closed() {
        assert!(menu_showing(HOME));
        assert!(menu_showing(EFFORT));
        assert!(menu_showing("  Switch model?\n  1. Yes, switch to Sonnet 5"));
        assert!(!menu_showing("❯ \n  ⏵⏵ auto mode on"));
    }

    #[test]
    fn codex_models_are_read_with_current_default_and_cursor() {
        let m = parse_codex_models(CX_MODELS).unwrap();
        assert_eq!(m.rows.len(), 7);
        assert_eq!(m.cursor, Some(2));
        let r = |n: usize| m.rows.iter().find(|r| r.number == n).unwrap();
        assert_eq!((r(1).label.as_str(), r(1).default, r(1).current), ("GPT-6.1-Sol", true, false));
        assert_eq!((r(2).label.as_str(), r(2).default, r(2).current), ("GPT-6-Astra", false, true));
        assert_eq!(r(7).description, "Older fast and efficient model.");
        assert!(parse_codex_models(CX_LEVELS).is_none(), "the level screen is not the model list");
    }

    #[test]
    fn codex_levels_name_their_model_and_find_a_level_by_name() {
        let l = parse_codex_levels(CX_LEVELS).unwrap();
        assert_eq!(l.model, "GPT-6.1-Sol");
        assert_eq!(l.cursor, Some(1));
        assert_eq!(l.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["Low", "Medium", "High", "Extra high", "More reasoning…"]);
        assert!(l.rows[0].default);
        assert_eq!(level_row(&l, "low"), Some(1));
        assert_eq!(level_row(&l, "Medium"), Some(2));
        assert_eq!(level_row(&l, "xhigh"), Some(4));
        assert_eq!(level_row(&l, "extra high"), Some(4));
        assert_eq!(level_row(&l, "max"), None);
        assert!(parse_codex_levels(CX_MODELS).is_none());
    }

    #[test]
    fn codex_screens_count_as_pickers_so_digits_are_never_sent_to_them() {
        assert!(picker_open(CX_MODELS));
        assert!(picker_open(CX_LEVELS));
        assert!(menu_showing(CX_LEVELS));
        assert!(!picker_open("› Ask Codex to do anything\n  GPT-6.1-Sol medium · ~/proj"));
        // Codex's approval prompt is a question to answer, not a picker.
        assert!(!picker_open("  Allow writing the file?\n› 1. Yes, proceed (y)\n  2. No"));
    }

    #[test]
    fn codex_confirmations_are_read_from_the_screen() {
        assert_eq!(
            codex_result("• Model changed to gpt-6-astra medium for this session only\n› Ask Codex"),
            "Model changed to gpt-6-astra medium for this session only"
        );
        assert_eq!(codex_result("nothing"), "Model changed");
    }

    #[test]
    fn the_result_line_is_what_claude_said() {
        assert_eq!(result_line("  ⎿  Set model to Sonnet 5 for this session only\n❯"), "Set model to Sonnet 5 for this session only");
        assert_eq!(result_line("  ⎿  Kept model as Opus in plan mode, else Sonnet"), "Kept model as Opus in plan mode, else Sonnet");
        assert_eq!(result_line("nothing useful"), "Model changed");
    }
}
