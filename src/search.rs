//! Searching a live session's whole tmux history, and the numbers the page uses to jump tmux's
//! copy-mode to a hit.
//!
//! The list comes from `capture-pane -J` (wrapped lines joined), the jump from tmux's own
//! `search-backward` + `search-again`, so tmux does the scrolling, wrapping and highlighting. The
//! two have to count matches the same way, and they do: newest first, rightmost first within a
//! line, non-overlapping, and case-insensitive only when the query has no ASCII capital (tmux's
//! "smart case"). Counting is by hit number `n`, so the jump is "the nth match from the bottom".

use serde::Serialize;

pub const MAX_QUERY: usize = 200;
/// Matches counted, and so reachable by a jump (each costs a `search-again`).
pub const MAX_COUNTED: usize = 1000;
/// Matches sent back to list.
pub const MAX_LISTED: usize = 200;
/// Characters of a line shown around a match.
const SNIPPET: usize = 160;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    /// This match counted from the bottom of the history; the newest is 1.
    pub n: usize,
    /// Lines between this one and the last line with text.
    pub up: usize,
    /// The line, cut to a window around the match (with … where it was cut).
    pub text: String,
    /// Where the match starts and how long it is, in characters of `text`.
    pub at: usize,
    pub len: usize,
}

#[derive(Debug, Serialize)]
pub struct Found {
    /// Matches in the whole history, up to `MAX_COUNTED`.
    pub total: usize,
    /// There are more than `total` (counting stopped).
    pub capped: bool,
    pub hits: Vec<Hit>,
}

/// A query worth searching for: not blank, not huge, no control characters.
pub fn clean(q: &str) -> Result<&str, &'static str> {
    if q.trim().is_empty() {
        return Err("empty search");
    }
    if q.len() > MAX_QUERY {
        return Err("search too long");
    }
    if q.chars().any(char::is_control) {
        return Err("search has control characters");
    }
    Ok(q)
}

/// tmux's smart case: ignore case unless the query has an ASCII capital.
pub(crate) fn ignores_case(q: &[char]) -> bool {
    !q.iter().any(char::is_ascii_uppercase)
}

fn same(a: char, b: char, fold: bool) -> bool {
    if fold { a.eq_ignore_ascii_case(&b) } else { a == b }
}

/// Where each non-overlapping match of `q` starts in `line`, left to right.
pub(crate) fn starts(line: &[char], q: &[char], fold: bool) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + q.len() <= line.len() {
        if q.iter().zip(&line[i..]).all(|(a, b)| same(*b, *a, fold)) {
            out.push(i);
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}

/// `text` around `at..at + len`, cut to `SNIPPET` characters.
pub(crate) fn window(line: &[char], at: usize, len: usize) -> (String, usize, usize) {
    let n = line.len();
    let (from, to) = if n <= SNIPPET { (0, n) } else {
        let from = at.saturating_sub(40).min(n - SNIPPET);
        (from, from + SNIPPET)
    };
    let mut text = String::new();
    let mut at_in = at - from;
    if from > 0 {
        text.push('…');
        at_in += 1;
    }
    text.extend(&line[from..to]);
    if to < n {
        text.push('…');
    }
    (text, at_in, len.min(to.saturating_sub(at)))
}

/// Every match in `capture` (the output of `capture-pane -p -J -S -`), newest first, with the
/// number a jump needs. Stops counting at `MAX_COUNTED`.
pub fn occurrences(capture: &str, query: &str) -> (Vec<Hit>, bool) {
    let q: Vec<char> = query.chars().collect();
    if q.is_empty() {
        return (Vec::new(), false);
    }
    let fold = ignores_case(&q);
    let lines: Vec<&str> = capture.trim_end().lines().collect();
    let mut hits = Vec::new();
    for (i, raw) in lines.iter().enumerate().rev() {
        let line: Vec<char> = raw.trim_end().chars().collect();
        for &at in starts(&line, &q, fold).iter().rev() {
            if hits.len() == MAX_COUNTED {
                return (hits, true);
            }
            let (text, at, len) = window(&line, at, q.len());
            hits.push(Hit { n: hits.len() + 1, up: lines.len() - 1 - i, text, at, len });
        }
    }
    (hits, false)
}

pub fn find(capture: &str, query: &str) -> Found {
    let (mut hits, capped) = occurrences(capture, query);
    let total = hits.len();
    hits.truncate(MAX_LISTED);
    Found { total, capped, hits }
}

/// The hit number to jump to. The history may have moved on since the list was fetched (new
/// output adds matches at the bottom and renumbers the rest), so if the line at `n` is no longer
/// the one that was tapped, use the nearest hit with that line instead.
pub fn resolve(hits: &[Hit], n: usize, text: Option<&str>) -> Option<usize> {
    let want = text?;
    if hits.get(n.wrapping_sub(1)).is_some_and(|h| h.text == want) {
        return Some(n);
    }
    hits.iter().filter(|h| h.text == want).min_by_key(|h| h.n.abs_diff(n)).map(|h| h.n)
}

/// `query` as a literal for tmux's regex search. A trailing `;` would end the tmux command and
/// let the rest run as another, and a leading `-` would read as a flag, so both are put in
/// brackets, as is every `-` for simplicity.
pub fn pattern(query: &str) -> String {
    let mut out = String::with_capacity(query.len() + 8);
    for c in query.chars() {
        match c {
            ';' => out.push_str("[;]"),
            '-' => out.push_str("[-]"),
            '\\' | '.' | '[' | ']' | '{' | '}' | '(' | ')' | '*' | '+' | '?' | '^' | '$' | '|' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HISTORY: &str = "line 1\na needle and another needle here\nxxxxxxneedle yyyy\nline 3\nNEEDLE upper\nline 4\n\n\n";

    #[test]
    fn counts_newest_first_and_rightmost_first_within_a_line() {
        let (hits, capped) = occurrences(HISTORY, "needle");
        assert!(!capped);
        let ns: Vec<(usize, &str)> = hits.iter().map(|h| (h.n, h.text.as_str())).collect();
        assert_eq!(
            ns,
            [(1, "NEEDLE upper"), (2, "xxxxxxneedle yyyy"), (3, "a needle and another needle here"), (4, "a needle and another needle here")]
        );
        // Within a line, the rightmost match is the lower number.
        assert!(hits[2].at > hits[3].at);
    }

    #[test]
    fn smart_case_matches_tmux() {
        assert_eq!(occurrences(HISTORY, "needle").0.len(), 4); // no capitals: ignores case
        let (hits, _) = occurrences(HISTORY, "NEEDLE");
        assert_eq!(hits.len(), 1); // a capital: exact
        assert_eq!(hits[0].text, "NEEDLE upper");
        assert_eq!(occurrences(HISTORY, "Needle").0.len(), 0);
    }

    #[test]
    fn case_folding_is_ascii_only_like_tmux() {
        assert_eq!(occurrences("Éa\néa", "éa").0.len(), 1);
    }

    #[test]
    fn matches_do_not_overlap() {
        let (hits, _) = occurrences("aaa\n", "aa");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].at, 0);
    }

    #[test]
    fn up_counts_lines_from_the_last_line_with_text() {
        let (hits, _) = occurrences(HISTORY, "line 4");
        assert_eq!(hits[0].up, 0); // trailing blank rows do not count
        let (hits, _) = occurrences(HISTORY, "line 1");
        assert_eq!(hits[0].up, 5);
    }

    #[test]
    fn long_lines_are_cut_around_the_match() {
        let line = format!("{}needle{}", "a".repeat(300), "b".repeat(300));
        let (hits, _) = occurrences(&line, "needle");
        let h = &hits[0];
        assert!(h.text.starts_with('…') && h.text.ends_with('…'));
        assert!(h.text.chars().count() <= SNIPPET + 2);
        let shown: String = h.text.chars().skip(h.at).take(h.len).collect();
        assert_eq!(shown, "needle");
    }

    #[test]
    fn a_match_near_the_end_keeps_its_window_full() {
        let line = format!("{}needle", "a".repeat(400));
        let h = &occurrences(&line, "needle").0[0];
        let shown: String = h.text.chars().skip(h.at).take(h.len).collect();
        assert_eq!(shown, "needle");
        assert!(!h.text.ends_with('…'));
    }

    #[test]
    fn counting_stops_at_the_cap() {
        let text = "x\n".repeat(MAX_COUNTED + 50);
        let (hits, capped) = occurrences(&text, "x");
        assert_eq!(hits.len(), MAX_COUNTED);
        assert!(capped);
        let found = find(&text, "x");
        assert_eq!((found.total, found.hits.len()), (MAX_COUNTED, MAX_LISTED));
    }

    #[test]
    fn resolve_follows_the_line_when_new_output_renumbers_hits() {
        // The list had "old line" at n=2; two newer matches have arrived since, so it is now n=4.
        let (hits, _) = occurrences("old line\nnew\nnew\n", "e");
        let old = hits.iter().find(|h| h.text == "old line").unwrap().n;
        assert_eq!(resolve(&hits, 1, Some("old line")), Some(old));
        assert_eq!(resolve(&hits, old, Some("old line")), Some(old));
        assert_eq!(resolve(&hits, 1, Some("gone")), None);
        assert_eq!(resolve(&hits, 1, None), None);
    }

    #[test]
    fn queries_are_checked() {
        assert!(clean("needle").is_ok());
        assert!(clean("  ").is_err());
        assert!(clean("").is_err());
        assert!(clean("a\nb").is_err());
        assert!(clean(&"x".repeat(MAX_QUERY + 1)).is_err());
    }

    #[test]
    fn the_tmux_pattern_is_literal_and_cannot_end_a_command() {
        assert_eq!(pattern("a.b"), r"a\.b");
        assert_eq!(pattern("c++"), r"c\+\+");
        assert_eq!(pattern("[x](y)"), r"\[x\]\(y\)");
        assert_eq!(pattern(r"back\slash"), r"back\\slash");
        assert_eq!(pattern("$5 ^x {1} a|b *"), r"\$5 \^x \{1\} a\|b \*");
        assert_eq!(pattern("x; kill-server;"), "x[;] kill[-]server[;]");
        assert!(!pattern("trailing;").ends_with(';'));
        assert!(!pattern("-flag").starts_with('-'));
        assert_eq!(pattern("Plain Text 123"), "Plain Text 123"); // capitals are kept: they pick tmux's case rule
    }
}
