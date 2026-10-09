//! `/resume` from the chat view. In both agents it is not a command that does something: it opens
//! an interactive list of earlier conversations, so the chat must not send it as a message.
//! Instead ttm opens the agent's own list, reads the rows off the screen, and presses the keys to
//! search, scroll and pick one. What the lists do was found by trying them (Claude Code 2.1.295,
//! Codex 0.161.0, the latter with a throwaway `CODEX_HOME`):
//!
//! - **Claude:** a search box, then two lines per session (title, then `17 minutes ago · HEAD ·
//!   297.3KB`). The title line holds the `❯` cursor, or `↑`/`↓` on the row cut off at an edge.
//!   `(9 of 50)` in the heading is the cursor's place in the whole list. `Ctrl+A` switches between
//!   this project and all projects. Typing searches, `Esc` clears the search and then closes.
//! - **Codex:** a filter line, a search line, then one line per session (`1d ago  Title`) with a
//!   `›` cursor, and `1 / 2 · 100%` in the divider. `Esc` goes back to the chat.
//! - In both, keys that arrive in a burst are dropped, so typing goes one character at a time and
//!   every move is checked against the screen. Picking a row is plain `Enter`, which only resumes
//!   that conversation and changes no setting.

use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use crate::{
    AppState,
    config::Agent,
    modelmenu::{ensure_ready, press, screen_text, type_text, wait_for},
};

/// More moves than any page needs; a guard against looping if a key never lands.
const MAX_MOVES: usize = 60;
/// Longest search ttm will type or erase.
const MAX_QUERY: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResumeRow {
    pub title: String,
    /// When it was last used, and for Claude the branch, size and folder.
    pub meta: String,
    /// Claude marks a conversation that is open in another window.
    pub open_elsewhere: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResumeList {
    pub rows: Vec<ResumeRow>,
    /// The row the highlight is on.
    #[serde(skip)]
    pub cursor: Option<usize>,
    pub more_above: bool,
    pub more_below: bool,
    /// The highlight's place in the whole list (1 is the newest) and the list's length.
    pub position: Option<usize>,
    pub total: Option<usize>,
    /// What is typed in the search box.
    pub query: String,
    /// Claude can switch between this project and all projects.
    pub can_scope: bool,
}

/// Is the agent's resume list on screen?
pub fn resume_open(screen: &str) -> bool {
    parse_claude(screen).is_some() || parse_codex(screen).is_some()
}

fn is_meta(line: &str) -> bool {
    line.contains(" ago · ")
}

/// Claude's list, read from a screen capture.
pub fn parse_claude(screen: &str) -> Option<ResumeList> {
    let lines: Vec<&str> = screen.lines().collect();
    let head = lines.iter().rposition(|l| l.trim().starts_with("Resume session"))?;
    let boxline = head + lines[head..].iter().position(|l| l.contains('⌕'))?;
    let typed = lines[boxline].split_once('⌕')?.1.trim().trim_end_matches('│').trim();
    let query = if typed == "Search…" { "" } else { typed };
    let start = boxline + 1 + lines[boxline..].iter().position(|l| l.trim_start().starts_with('╰'))?;
    let end = start
        + lines[start..].iter().position(|l| {
            let t = l.trim_start();
            t.starts_with("Ctrl+A to") || t.starts_with("Type to Search") || t.starts_with("Type to search")
        })?;
    let body = &lines[start..end];

    let mut rows = Vec::new();
    let (mut cursor, mut above, mut below) = (None, false, false);
    for (i, line) in body.iter().enumerate() {
        let next = body.get(i + 1).map(|n| n.trim());
        if is_meta(line) || !next.is_some_and(is_meta) {
            continue; // a meta line, or a project heading
        }
        let mut title = line.trim();
        for (mark, set) in [('❯', 0), ('↑', 1), ('↓', 2)] {
            if let Some(rest) = title.strip_prefix(mark) {
                title = rest.trim_start();
                match set {
                    0 => cursor = Some(rows.len()),
                    1 => above = true,
                    _ => below = true,
                }
            }
        }
        let (title, open_elsewhere) = match title.split_once('⧉') {
            Some((t, _)) => (t.trim(), true),
            None => (title, false),
        };
        rows.push(ResumeRow { title: title.to_owned(), meta: next.unwrap_or_default().to_owned(), open_elsewhere });
    }
    // `Resume session (9 of 50)`
    let place = lines[head].split_once('(').and_then(|(_, r)| r.split_once(')')).map(|(p, _)| p);
    let (position, total) = place
        .and_then(|p| p.split_once(" of "))
        .map_or((None, None), |(a, b)| (a.trim().parse().ok(), b.trim().parse().ok()));
    // Without the count (the current project is short) the list is just what is on screen.
    let total = total.or(Some(rows.len()));
    let position = position.or(cursor.map(|c| c + 1));
    Some(ResumeList { rows, cursor, more_above: above, more_below: below, position, total, query: query.to_owned(), can_scope: true })
}

/// Codex's list, read from a screen capture.
pub fn parse_codex(screen: &str) -> Option<ResumeList> {
    let lines: Vec<&str> = screen.lines().collect();
    let head = lines.iter().rposition(|l| l.trim() == "Resume a previous session")?;
    let filter = head + lines[head..].iter().position(|l| l.trim_start().starts_with("Filter:"))?;
    // The search line is the first one below the filter line; a blank line may sit between.
    let search_at = filter + 1 + lines[filter + 1..].iter().position(|l| !l.trim().is_empty())?;
    let search = lines[search_at].trim();
    let query = if search.starts_with("Type to search") { "" } else { search.trim_start_matches("Search:").trim() };
    let divider = search_at + 1 + lines[search_at + 1..].iter().position(|l| l.trim_start().starts_with('─'))?;

    let mut rows = Vec::new();
    let mut cursor = None;
    for line in &lines[search_at + 1..divider] {
        let mut t = line.trim();
        let on = t.starts_with('›');
        t = t.trim_start_matches('›').trim_start();
        let mut words = t.split_whitespace();
        let (Some(n), Some("ago")) = (words.next(), words.next()) else { continue };
        if !n.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let title = t.split_once("ago").map_or("", |(_, r)| r).trim();
        if on {
            cursor = Some(rows.len());
        }
        rows.push(ResumeRow { title: title.to_owned(), meta: format!("{n} ago"), open_elsewhere: false });
    }
    // `── 1 / 2 · 100% ──`
    let counts = lines[divider].trim_matches(|c: char| c == '─' || c.is_whitespace());
    let (position, total) = counts
        .split_once(" · ")
        .and_then(|(c, _)| c.split_once(" / "))
        .map_or((None, None), |(a, b)| (a.trim().parse().ok().filter(|n| *n > 0), b.trim().parse().ok()));
    Some(ResumeList { rows, cursor, more_above: false, more_below: false, position, total, query: query.to_owned(), can_scope: false })
}

fn parse(agent: Agent, screen: &str) -> Option<ResumeList> {
    match agent {
        Agent::Claude => parse_claude(screen),
        Agent::Codex => parse_codex(screen),
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

async fn read(id: &str, agent: Agent) -> Result<ResumeList> {
    parse(agent, &screen_text(id).await?).context("the resume list is not open (open it again)")
}

/// Open `/resume` and read what the agent shows. The list stays open until a pick or a close.
pub async fn open(st: &Arc<AppState>, id: &str) -> Result<ResumeList> {
    let agent = agent_of(st, id)?;
    ensure_ready(st, id, agent).await?;
    type_text(id, "/resume").await?;
    press(id, "Enter").await?;
    wait_for(id, "the session list", |s| parse(agent, s).is_some()).await?;
    // The rows can arrive a moment after the frame.
    for _ in 0..4 {
        let list = read(id, agent).await?;
        if !list.rows.is_empty() {
            return Ok(list);
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    read(id, agent).await
}

/// Replace what is in the search box: erase `previous`, then type `query`.
pub async fn search(st: &Arc<AppState>, id: &str, query: &str, previous: usize) -> Result<ResumeList> {
    let agent = agent_of(st, id)?;
    ensure!(query.chars().count() <= MAX_QUERY && previous <= MAX_QUERY, "that search is too long");
    ensure!(!query.chars().any(char::is_control), "that search has characters the list can't take");
    read(id, agent).await?;
    for _ in 0..previous {
        press(id, "BSpace").await?;
    }
    for ch in query.chars() {
        type_text(id, &ch.to_string()).await?;
    }
    read(id, agent).await
}

/// Claude only: switch between this project and all projects.
pub async fn scope(st: &Arc<AppState>, id: &str) -> Result<ResumeList> {
    let agent = agent_of(st, id)?;
    ensure!(agent == Agent::Claude, "only Claude Code can switch between projects here");
    read(id, agent).await?;
    press(id, "C-a").await?;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    read(id, agent).await
}

/// Scroll by about a page, towards older sessions or back. Moves the highlight and checks its place
/// after every key, because the list scrolls with it. While a search is typed, Claude shows no
/// highlight, so the first Down moves from the search box into the list.
pub async fn page(st: &Arc<AppState>, id: &str, down: bool) -> Result<ResumeList> {
    let agent = agent_of(st, id)?;
    let mut now = read(id, agent).await?;
    if now.cursor.is_none() && !now.rows.is_empty() {
        press(id, "Down").await?;
        now = read(id, agent).await?;
    }
    let (Some(at), Some(on)) = (now.position, now.cursor) else { return Ok(now) };
    // The window only scrolls once the highlight passes its edge. From the last row shown, a
    // window further down ends exactly one window past it; from the first row shown, one window
    // further up starts exactly one window before it. So no row is skipped between pages.
    let len = now.rows.len().max(1);
    let first = at.saturating_sub(on).max(1); // the place of the first row shown
    let last_shown = first + len - 1;
    // The total is exact for a plain list; with a search it is only what shows, so don't clamp to it.
    let limit = if now.more_below { usize::MAX } else { now.total.unwrap_or(usize::MAX) };
    let target = if down { last_shown.saturating_add(len).min(limit) } else { first.saturating_sub(len).max(1) };
    let mut last = None;
    for _ in 0..MAX_MOVES {
        let here = read(id, agent).await?.position.context("lost the session list")?;
        if here == target || last == Some(here) {
            break; // arrived, or the key did nothing (the end of the list)
        }
        last = Some(here);
        press(id, if here < target { "Down" } else { "Up" }).await?;
    }
    read(id, agent).await
}

/// The meta line without its "17 minutes ago" part, which changes between reads.
fn stable_meta(meta: &str) -> &str {
    meta.split_once(" · ").map_or("", |(_, rest)| rest)
}

/// Which row the caller meant: the one with that title and branch/size/folder, or if several look
/// the same, the one at the position they saw it in.
fn find_row(rows: &[ResumeRow], title: &str, meta: &str, index: usize) -> Option<usize> {
    let same: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].title == title && stable_meta(&rows[i].meta) == stable_meta(meta))
        .collect();
    match same.as_slice() {
        [one] => Some(*one),
        [] => None,
        many => many.contains(&index).then_some(index),
    }
}

/// Move the highlight to the row the caller picked, then press Enter to resume it.
pub async fn choose(id: &str, agent: Agent, title: &str, meta: &str, index: usize) -> Result<String> {
    for _ in 0..MAX_MOVES {
        let list = read(id, agent).await?;
        let want = find_row(&list.rows, title, meta, index).context("that session is no longer in the list (the list changed)")?;
        let Some(at) = list.cursor else {
            // Typing a search leaves no highlight; the first Down moves into the list.
            press(id, "Down").await?;
            continue;
        };
        if at == want {
            press(id, "Enter").await?;
            let gone = wait_for(id, "the list to close", |s| parse(agent, s).is_none()).await;
            ensure!(gone.is_ok(), "the agent did not open that session");
            return Ok(format!("Resuming {title}"));
        }
        press(id, if at < want { "Down" } else { "Up" }).await?;
    }
    bail!("could not move to that session")
}

pub async fn choose_for(st: &Arc<AppState>, id: &str, title: &str, meta: &str, index: usize) -> Result<String> {
    let agent = agent_of(st, id)?;
    choose(id, agent, title, meta, index).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = include_str!("testdata/resume_claude.txt");
    const CLAUDE_ALL: &str = include_str!("testdata/resume_claude_all.txt");
    const CODEX: &str = include_str!("testdata/resume_codex.txt");
    const CODEX_SEARCH: &str = include_str!("testdata/resume_codex_search.txt");

    #[test]
    fn reads_claudes_list_for_this_project() {
        let l = parse_claude(CLAUDE).expect("the list");
        assert_eq!(l.rows.len(), 2);
        assert_eq!(l.rows[0].title, "Rust todo CLI");
        assert_eq!(l.rows[0].meta, "17 minutes ago · HEAD · 297.3KB");
        assert_eq!(l.rows[1].title, "(session)");
        assert_eq!(l.cursor, Some(0));
        assert_eq!(l.query, "");
        assert!(!l.more_below);
        assert_eq!((l.position, l.total), (Some(1), Some(2)));
    }

    #[test]
    fn reads_claudes_list_for_all_projects() {
        let l = parse_claude(CLAUDE_ALL).expect("the list");
        assert!(l.rows.len() >= 8, "{} rows", l.rows.len());
        assert_eq!(l.rows[0].title, "fix-login-redirect");
        assert!(l.rows[0].meta.ends_with("/Users/me/code/app"));
        assert_eq!(l.rows[1].title, "tidy-onboarding-copy");
        assert!(l.rows[1].open_elsewhere, "the ⧉ marker means it is open elsewhere");
        assert_eq!(l.cursor, Some(0));
        assert!(l.more_below, "the last row has the ↓ marker");
        assert_eq!((l.position, l.total), (Some(1), Some(50)));
    }

    #[test]
    fn reads_codexs_list() {
        let l = parse_codex(CODEX).expect("the list");
        assert_eq!(l.rows.len(), 2);
        assert_eq!(l.rows[0].title, "Monitor service logs and usage");
        assert_eq!(l.rows[0].meta, "1d ago");
        assert!(l.rows[1].title.starts_with("I want to build a script"));
        assert_eq!(l.cursor, Some(0));
        assert_eq!((l.position, l.total), (Some(1), Some(2)));
        assert_eq!(l.query, "");
        assert!(!l.can_scope);
    }

    fn row(title: &str, meta: &str) -> ResumeRow {
        ResumeRow { title: title.to_owned(), meta: meta.to_owned(), open_elsewhere: false }
    }

    #[test]
    fn a_row_is_found_even_after_its_age_changes() {
        let rows = [row("a", "5 minutes ago · main · 1KB"), row("b", "6 minutes ago · main · 2KB")];
        assert_eq!(find_row(&rows, "b", "5 minutes ago · main · 2KB", 0), Some(1));
        assert_eq!(find_row(&rows, "c", "", 0), None);
    }

    #[test]
    fn rows_that_look_the_same_are_told_apart_by_position() {
        let rows = [row("/clear", "1d ago"), row("/clear", "1d ago"), row("x", "2d ago")];
        assert_eq!(find_row(&rows, "/clear", "1d ago", 1), Some(1));
        assert_eq!(find_row(&rows, "/clear", "1d ago", 2), None);
    }

    #[test]
    fn reads_codexs_typed_search() {
        let l = parse_codex(CODEX_SEARCH).expect("the list");
        assert_eq!(l.query, "py");
        assert_eq!(l.rows.len(), 1);
        assert_eq!((l.position, l.total), (Some(1), Some(1)));
    }

    #[test]
    fn the_search_text_is_read_back() {
        let s = CLAUDE.replace("⌕ Search…", "⌕ ru     ");
        assert_eq!(parse_claude(&s).map(|l| l.query), Some("ru".to_owned()));
    }

    #[test]
    fn other_screens_are_not_the_list() {
        assert!(!resume_open("❯ hello\n  ⏵⏵ auto mode on"));
        assert!(!resume_open("Select model\n❯ 1. Default (recommended)"));
        assert!(resume_open(CLAUDE) && resume_open(CODEX));
    }
}
