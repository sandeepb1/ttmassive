//! Your messages to an agent, found again on the terminal, so the page can list them and jump to
//! each one in tmux's history.
//!
//! The transcript says what you typed; the terminal says where it is. A message is looked for by
//! its first line, and counted the way tmux's own search counts (see `search.rs`), so the page can
//! reach it with the same jump as a search. The agent draws a submitted message in its own way, so
//! no prompt glyph is assumed: a match counts as your message when only a few symbols come before
//! it on its line (`> `, `❯ `, `│ > `), which keeps the agent quoting you from being mistaken for
//! you. Messages are matched from the newest end, so one that cannot be found (a long paste the
//! agent shows as `[Pasted text]`, or history that was cleared) leaves only itself unplaced.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::search;

/// Newest messages listed.
const MAX_TURNS: usize = 40;
/// How much of a message's first line is searched for. Enough to be distinctive, short enough
/// to survive the agent wrapping or trimming it.
const SNIPPET_CHARS: usize = 60;
const SNIPPET_BYTES: usize = 180;
/// A listed message's preview.
const PREVIEW_CHARS: usize = 120;
/// Symbols allowed before a match for it to look like a prompt line.
const MAX_LEAD_SYMBOLS: usize = 4;

/// A message you sent, from the transcript.
pub struct UserMsg {
    pub id: String,
    pub ts: Option<i64>,
    pub text: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Turn {
    pub id: String,
    pub ts: Option<i64>,
    /// The message, on one line, for the list.
    pub preview: String,
    /// What to search for to reach it: pass as `q` to the search jump.
    pub snippet: String,
    /// Its match number counted from the newest, for the jump. None: not found on the terminal.
    pub n: Option<usize>,
    /// The line it is on, as the jump expects it back (see `search::resolve`).
    pub text: Option<String>,
    /// Lines between it and the last line with text.
    pub up: Option<usize>,
}

/// The first line of `text` as a search string, or None if there is nothing distinctive to find.
pub fn snippet(text: &str) -> Option<String> {
    let first = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut out = String::new();
    for c in first.chars().take(SNIPPET_CHARS) {
        if out.len() + c.len_utf8() > SNIPPET_BYTES {
            break;
        }
        out.push(if c.is_control() { ' ' } else { c });
    }
    let out = out.trim_end().to_owned();
    (out.chars().filter(|c| !c.is_whitespace()).count() >= 3).then_some(out)
}

fn preview(text: &str) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match one.char_indices().nth(PREVIEW_CHARS) {
        Some((i, _)) => format!("{}…", &one[..i]),
        None => one,
    }
}

/// Only a few symbols before the match: the agent's prompt mark and box edge, not prose.
fn looks_like_prompt(lead: &[char]) -> bool {
    lead.iter().all(|c| !c.is_alphanumeric()) && lead.iter().filter(|c| !c.is_whitespace()).count() <= MAX_LEAD_SYMBOLS
}

struct Cand {
    n: usize,
    prompt: bool,
    text: String,
    up: usize,
}

/// One distinct snippet, the messages that share it (newest first) and what was found for it.
struct Group {
    chars: Vec<char>,
    lower: String,
    exact: String,
    fold: bool,
    /// Indices into the newest-first list of messages.
    turns: Vec<usize>,
    cands: Vec<Cand>,
    seen: usize,
    prompts: usize,
    done: bool,
}

/// Place your `msgs` (oldest first, as in the transcript) on `capture` (`capture-pane -p -J -S -`).
/// Returns the newest `MAX_TURNS`, newest first.
pub fn locate(capture: &str, msgs: &[UserMsg]) -> Vec<Turn> {
    let newest: Vec<&UserMsg> = msgs.iter().rev().take(MAX_TURNS).collect();
    let snippets: Vec<Option<String>> = newest.iter().map(|m| snippet(&m.text)).collect();

    let mut groups: BTreeMap<&str, Group> = BTreeMap::new();
    for (i, s) in snippets.iter().enumerate() {
        let Some(s) = s else { continue };
        groups
            .entry(s.as_str())
            .or_insert_with(|| {
                let chars: Vec<char> = s.chars().collect();
                Group {
                    fold: search::ignores_case(&chars),
                    chars,
                    lower: s.to_ascii_lowercase(),
                    exact: s.clone(),
                    turns: Vec::new(),
                    cands: Vec::new(),
                    seen: 0,
                    prompts: 0,
                    done: false,
                }
            })
            .turns
            .push(i);
    }

    // One pass from the newest line up, counting each snippet's matches the way tmux does.
    let lines: Vec<&str> = capture.trim_end().lines().collect();
    for (i, raw) in lines.iter().enumerate().rev() {
        if groups.values().all(|g| g.done) {
            break;
        }
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let mut chars: Option<Vec<char>> = None;
        for g in groups.values_mut().filter(|g| !g.done) {
            let hit = if g.fold { lower.contains(&g.lower) } else { line.contains(&g.exact) };
            if !hit {
                continue;
            }
            let chars = chars.get_or_insert_with(|| line.chars().collect());
            for &at in search::starts(chars, &g.chars, g.fold).iter().rev() {
                if g.seen == search::MAX_COUNTED {
                    g.done = true;
                    break;
                }
                g.seen += 1;
                let prompt = looks_like_prompt(&chars[..at]);
                let (text, _, _) = search::window(chars, at, g.chars.len());
                g.cands.push(Cand { n: g.seen, prompt, text, up: lines.len() - 1 - i });
                if prompt {
                    g.prompts += 1;
                    if g.prompts >= g.turns.len() {
                        g.done = true; // every message that shares this snippet has a place
                        break;
                    }
                }
            }
        }
    }

    // Hand out matches newest to newest. Prefer the ones that look like a prompt line; if none do
    // (an agent that draws your message some other way), any match will do.
    let mut placed: Vec<Option<(usize, String, usize)>> = vec![None; newest.len()];
    for g in groups.values() {
        let prompts: Vec<&Cand> = g.cands.iter().filter(|c| c.prompt).collect();
        let pool: Vec<&Cand> = if prompts.is_empty() { g.cands.iter().collect() } else { prompts };
        for (k, &turn) in g.turns.iter().enumerate() {
            if let Some(c) = pool.get(k) {
                placed[turn] = Some((c.n, c.text.clone(), c.up));
            }
        }
    }

    newest
        .iter()
        .zip(snippets)
        .zip(placed)
        .filter_map(|((m, snip), at)| {
            let snip = snip?;
            let (n, text, up) = match at {
                Some((n, t, u)) => (Some(n), Some(t), Some(u)),
                None => (None, None, None),
            };
            Some(Turn { id: m.id.clone(), ts: m.ts, preview: preview(&m.text), snippet: snip, n, text, up })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, text: &str) -> UserMsg {
        UserMsg { id: id.into(), ts: None, text: text.into() }
    }

    #[test]
    fn snippet_is_the_first_line_cleaned_and_cut() {
        assert_eq!(snippet("fix the parser\nand the lexer").as_deref(), Some("fix the parser"));
        assert_eq!(snippet("\n\n  hello there  ").as_deref(), Some("hello there"));
        assert_eq!(snippet("ab\tcd").as_deref(), Some("ab cd")); // a control character would be refused by the search
        let long = snippet(&"word ".repeat(40)).unwrap();
        assert!(long.chars().count() <= SNIPPET_CHARS && !long.ends_with(' '));
        let wide = snippet(&"字".repeat(100)).unwrap();
        assert!(wide.len() <= SNIPPET_BYTES, "cut by bytes too");
    }

    #[test]
    fn snippet_needs_something_distinctive() {
        assert_eq!(snippet(""), None);
        assert_eq!(snippet("   \n  "), None);
        assert_eq!(snippet("ok"), None);
        assert_eq!(snippet("yes"), Some("yes".into()));
    }

    #[test]
    fn preview_is_one_short_line() {
        assert_eq!(preview("a\n\n  b   c"), "a b c");
        assert!(preview(&"x ".repeat(200)).chars().count() <= PREVIEW_CHARS + 1);
    }

    #[test]
    fn prompt_lines_are_told_from_prose() {
        let l = |s: &str| looks_like_prompt(&s.chars().collect::<Vec<_>>());
        assert!(l(""));
        assert!(l("> "));
        assert!(l("❯ "));
        assert!(l("│ > "));
        assert!(l("  › "));
        assert!(!l("I will "));
        assert!(!l("you said: "));
        assert!(!l("> > > > > ")); // too many symbols
    }

    const SCREEN: &str = "\
> fix the bug in the parser
  Looking at it.
I will fix the bug in the parser now.
> run the tests
  All passing.
> fix the bug in the parser
  Done again.
";

    #[test]
    fn messages_are_placed_newest_first_and_counted_like_tmux() {
        let msgs = [msg("a", "fix the bug in the parser"), msg("b", "run the tests"), msg("c", "fix the bug in the parser")];
        let t = locate(SCREEN, &msgs);
        assert_eq!(t.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["c", "b", "a"]);
        // "fix the bug…" is on 3 lines (the middle one is the agent quoting you). Counted from the
        // newest: the last prompt is 1, the quote is 2, the first prompt is 3.
        assert_eq!(t[0].n, Some(1));
        assert_eq!(t[2].n, Some(3), "the first message skips the agent's quote");
        assert_eq!(t[1].n, Some(1));
        assert_eq!(t[0].text.as_deref(), Some("> fix the bug in the parser"));
        assert_eq!(t[0].up, Some(1)); // "  Done again." is below it
    }

    #[test]
    fn a_message_not_on_screen_is_left_unplaced_and_does_not_shift_others() {
        let msgs = [msg("old", "something long ago"), msg("a", "run the tests"), msg("pasted", "[the pasted thing shown differently]")];
        let t = locate(SCREEN, &msgs);
        assert_eq!(t[0].id, "pasted");
        assert_eq!(t[0].n, None);
        assert_eq!(t[1].n, Some(1)); // "run the tests" is still found
        assert_eq!(t[2].n, None);
        assert!(t[0].text.is_none() && t[0].up.is_none());
    }

    #[test]
    fn repeated_messages_run_out_of_matches_from_the_old_end() {
        // Three identical messages, but the terminal history only holds the two newest.
        let screen = "> go on\nok\n> go on\nok\n";
        let msgs = [msg("1", "go on"), msg("2", "go on"), msg("3", "go on")];
        let t = locate(screen, &msgs);
        assert_eq!([t[0].n, t[1].n, t[2].n], [Some(1), Some(2), None]);
    }

    #[test]
    fn a_decorated_prompt_box_still_counts_as_a_prompt() {
        let screen = "│ > deploy the thing\nplease deploy the thing now\n";
        let t = locate(screen, &[msg("a", "deploy the thing")]);
        assert_eq!(t[0].n, Some(2), "the boxed prompt is the older match");
    }

    #[test]
    fn any_match_will_do_if_nothing_looks_like_a_prompt() {
        let screen = "You: refactor the module\nok\n";
        let t = locate(screen, &[msg("a", "refactor the module")]);
        assert_eq!(t[0].n, Some(1));
    }

    #[test]
    fn case_follows_tmux_smart_case() {
        // No capital in the message: matches regardless of case. A capital: exact.
        assert_eq!(locate("> FIX THE BUG\n", &[msg("a", "fix the bug")])[0].n, Some(1));
        assert_eq!(locate("> fix the bug\n", &[msg("a", "Fix the bug")])[0].n, None);
        assert_eq!(locate("> Fix the bug\n", &[msg("a", "Fix the bug")])[0].n, Some(1));
    }

    #[test]
    fn numbers_agree_with_the_search_endpoint() {
        // The jump re-counts with `search::occurrences`; a turn's n must be one of its hits.
        let msgs = [msg("a", "fix the bug in the parser"), msg("b", "run the tests"), msg("c", "fix the bug in the parser")];
        for t in locate(SCREEN, &msgs) {
            let (hits, _) = search::occurrences(SCREEN, &t.snippet);
            let n = t.n.unwrap();
            assert_eq!(hits[n - 1].text, t.text.clone().unwrap());
        }
    }

    #[test]
    fn only_the_newest_messages_are_listed() {
        let msgs: Vec<UserMsg> = (0..MAX_TURNS + 10).map(|i| msg(&i.to_string(), &format!("message number {i}"))).collect();
        let t = locate("", &msgs);
        assert_eq!(t.len(), MAX_TURNS);
        assert_eq!(t[0].id, (MAX_TURNS + 9).to_string());
        assert!(t.iter().all(|t| t.n.is_none()));
    }

    #[test]
    fn messages_with_nothing_to_search_for_are_dropped() {
        let t = locate("> hello there\n", &[msg("a", "ok"), msg("b", "hello there")]);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].id, "b");
    }

    #[test]
    fn only_the_prompt_looking_match_on_a_line_is_a_message() {
        // "> dup dup": tmux counts both (right one first), but only the left one is after just `> `.
        let t = locate("> dup dup\n", &[msg("a", "dup"), msg("b", "dup")]);
        assert_eq!([t[0].n, t[1].n], [Some(2), None]);
    }
}
