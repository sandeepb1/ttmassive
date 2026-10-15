//! Reads a captured terminal screen and works out what an agent is doing and,
//! when it is asking a question, what the answer choices are and which keys
//! select each one. Pure text in, structure out, so it is easy to test.
//!
//! Heuristics are tuned for the numbered selector menus Claude Code and Codex
//! draw ("❯ 1. Yes" / "  2. No (esc)") and for inline `[y/N]` prompts. They are
//! best effort: a prompt is only reported when it sits at the bottom of the
//! screen and looks like a question, and answering re-checks the screen first.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How many non-empty lines from the bottom of the screen are considered.
const WINDOW: usize = 40;
/// A menu must end within this many non-empty lines of the bottom.
const MENU_MAX_FROM_BOTTOM: usize = 12;
const MAX_CONTEXT: usize = 4;
/// Footer lines allowed after a live menu.
const MAX_TRAILING: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    /// Busy: output says it can be interrupted.
    Working,
    /// Blocked on a question.
    Waiting,
    /// Showing an input prompt, ready for the next instruction.
    Idle,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// Numbered options, usually with a highlighted selection.
    Menu,
    /// Inline `[y/N]` style question.
    YesNo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// 1-based position.
    pub index: usize,
    pub label: String,
    /// Whether the highlight cursor is on this option (menus only).
    pub selected: bool,
    /// Hotkey the screen advertises, e.g. `y`, `a`, `esc`.
    pub hotkey: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompt {
    pub kind: PromptKind,
    pub question: String,
    /// Up to a few non-empty lines shown above the question (the command, the diff header, ...).
    pub context: Vec<String>,
    pub choices: Vec<Choice>,
    /// Stable hash of the question and choices; answering must quote it.
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
    pub activity: Activity,
    pub prompt: Option<Prompt>,
    /// The last few non-empty lines, for a quick glance.
    pub tail: Vec<String>,
}

/// One step of keystrokes to send to tmux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A tmux key name (`Enter`, `Down`, `Escape`, ...).
    Key(&'static str),
    /// Literal text.
    Text(String),
}

const BOX_EDGE: &[char] = &['│', '┃', '║', '╎', '┆', '┇', '┊', '┋', ' ', '\t'];
const BOX_ONLY: &[char] = &[
    '─', '━', '═', '╭', '╮', '╰', '╯', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼', '│', '┃', '║', ' ', '\t',
];
const SELECTORS: &[char] = &['❯', '›', '>', '▶', '➤', '→', '▸', '●'];

/// Strip box-drawing borders and surrounding spaces; pure borders become empty.
fn clean(raw: &str) -> String {
    let t = raw.trim_end();
    if t.chars().all(|c| BOX_ONLY.contains(&c)) {
        return String::new();
    }
    t.trim_matches(BOX_EDGE).to_owned()
}

struct Opt {
    marker: bool,
    number: usize,
    label: String,
}

/// Parse `[marker] N. label` / `N) label`.
fn parse_option(line: &str) -> Option<Opt> {
    let mut rest = line.trim_start();
    let mut marker = false;
    if let Some(c) = rest.chars().next()
        && SELECTORS.contains(&c)
    {
        marker = true;
        rest = rest[c.len_utf8()..].trim_start();
    }
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let after = &rest[digits.len()..];
    let after = after.strip_prefix('.').or_else(|| after.strip_prefix(')'))?;
    let label = after.strip_prefix(' ')?.trim();
    if label.is_empty() {
        return None;
    }
    Some(Opt { marker, number: digits.parse().ok()?, label: label.to_owned() })
}

/// Pull a trailing `(y)` / `(esc)` hotkey hint off a label.
fn hotkey_of(label: &str) -> Option<String> {
    let inner = label.trim_end().strip_suffix(')')?.rsplit_once('(')?.1.trim();
    match inner.to_ascii_lowercase().as_str() {
        "esc" => Some("esc".into()),
        s if s.len() == 1 && s.chars().all(|c| c.is_ascii_alphabetic()) => Some(s.into()),
        _ => None,
    }
}

/// Footer lines TUIs print under a menu ("Press enter to confirm", "Esc to cancel · Tab to ...").
fn is_hint(line: &str) -> bool {
    const WORDS: &[&str] = &[
        "enter", "esc", "confirm", "select", "navigate", "cancel", "tab", "shortcut", "space", "ctrl", "arrow", "↑", "↓",
        "choose", "go back",
    ];
    let l = line.to_ascii_lowercase();
    l.chars().count() <= 100 && WORDS.iter().any(|w| l.contains(w))
}

fn looks_like_question(q: &str) -> bool {
    let l = q.to_ascii_lowercase();
    q.trim_end().ends_with('?')
        || ["proceed", "continue", "allow", "approve", "choose", "select", "would you like", "do you want"]
            .iter()
            .any(|w| l.contains(w))
}

fn prompt_id(question: &str, choices: &[Choice]) -> String {
    let mut h = Sha256::new();
    h.update(question.as_bytes());
    for c in choices {
        h.update([0]);
        h.update(c.label.as_bytes());
    }
    h.finalize()[..6].iter().map(|b| format!("{b:02x}")).collect()
}

fn find_menu(lines: &[String]) -> Option<Prompt> {
    // Collect runs of consecutive numbered options, keeping the last good one.
    let mut best: Option<(usize, usize)> = None; // [start, end) into `lines`
    let mut i = 0;
    while i < lines.len() {
        let Some(first) = parse_option(&lines[i]) else {
            i += 1;
            continue;
        };
        let mut j = i + 1;
        let mut expect = first.number + 1;
        while j < lines.len() {
            match parse_option(&lines[j]) {
                Some(o) if o.number == expect => {
                    expect += 1;
                    j += 1;
                }
                _ => break,
            }
        }
        if first.number == 1 && j - i >= 2 {
            best = Some((i, j));
        }
        i = j.max(i + 1);
    }
    let (start, end) = best?;
    if lines.len() - end > MENU_MAX_FROM_BOTTOM {
        return None; // scrolled-up output, not a live prompt
    }
    // A live menu is the last thing on screen, optionally followed by a footer hint. Anything
    // else after it (output, a typed answer, a new prompt) means it was already answered.
    let trailing = &lines[end..];
    if trailing.len() > MAX_TRAILING || !trailing.iter().all(|l| is_hint(l)) {
        return None;
    }

    let opts: Vec<Opt> = lines[start..end].iter().filter_map(|l| parse_option(l)).collect();
    let above: Vec<&String> = lines[..start].iter().rev().collect();
    // The question is usually the line right above the options, but Codex puts the
    // command there and the question above it: prefer a nearby line that reads like one.
    let q_at = above.iter().take(6).position(|l| looks_like_question(l)).unwrap_or(0);
    let question = above.get(q_at).map(|s| s.to_string()).unwrap_or_default();
    let has_marker = opts.iter().any(|o| o.marker);
    if !has_marker && !looks_like_question(&question) {
        return None; // probably a numbered list in ordinary output
    }
    let mut context: Vec<String> = above
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != q_at)
        .take(MAX_CONTEXT)
        .map(|(_, l)| l.to_string())
        .collect();
    context.reverse();
    let choices: Vec<Choice> = opts
        .into_iter()
        .map(|o| Choice { index: o.number, selected: o.marker, hotkey: hotkey_of(&o.label), label: o.label })
        .collect();
    Some(Prompt { kind: PromptKind::Menu, id: prompt_id(&question, &choices), question, context, choices })
}

fn find_yes_no(lines: &[String]) -> Option<Prompt> {
    const MARKS: &[&str] = &["[y/n]", "(y/n)", "[yes/no]", "(yes/no)", "[y/n/a]", "(y/n/a)"];
    // Still waiting means the marker is the last thing on the line: once an answer is typed
    // after it ("... [y/N] y") the question is over.
    let (pos, line) = lines.iter().enumerate().rev().take(2).find(|(_, l)| {
        let low = l.to_ascii_lowercase();
        let low = low.trim_end_matches([' ', ':', '?']);
        MARKS.iter().any(|m| low.ends_with(m))
    })?;
    let choices = vec![
        Choice { index: 1, label: "Yes".into(), selected: false, hotkey: Some("y".into()) },
        Choice { index: 2, label: "No".into(), selected: false, hotkey: Some("n".into()) },
    ];
    let context: Vec<String> =
        lines[..pos].iter().rev().take(MAX_CONTEXT).rev().cloned().collect();
    Some(Prompt {
        kind: PromptKind::YesNo,
        id: prompt_id(line, &choices),
        question: line.clone(),
        context,
        choices,
    })
}

/// An unnumbered selector menu: a cursor line and its siblings above an "Enter to confirm · Esc to cancel" footer.
/// Claude Code's folder-trust dialog is one ("❯ No, exit" / "Yes, I trust this folder"). There are no numbers to
/// anchor on and `clean` has stripped the indentation, so this is deliberately narrow: the options are the block
/// of 2 to 6 short lines directly above that exact footer, separated from the text above by a blank line, with
/// exactly one carrying the cursor. A shell prompt or an ordinary list has no such footer and is left alone.
fn find_plain_menu(screen: &str) -> Option<Prompt> {
    let lines: Vec<String> = screen.lines().map(clean).collect(); // blank lines stay: they bound the block
    let footer = lines.iter().rposition(|l| !l.is_empty())?;
    let f = lines[footer].to_ascii_lowercase();
    if !(f.contains("enter to confirm") && f.contains("esc to cancel")) {
        return None;
    }
    let mut hi = footer;
    while hi > 0 && lines[hi - 1].is_empty() {
        hi -= 1;
    }
    let mut lo = hi;
    while lo > 0 && !lines[lo - 1].is_empty() {
        lo -= 1;
    }
    let block = &lines[lo..hi];
    if !(2..=6).contains(&block.len()) || block.iter().any(|l| l.chars().count() > 80) {
        return None;
    }
    let is_cursor = |l: &String| l.chars().next().is_some_and(|c| SELECTORS.contains(&c));
    if block.iter().filter(|l| is_cursor(l)).count() != 1 {
        return None;
    }
    let choices: Vec<Choice> = block
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let selected = is_cursor(l);
            let label = if selected { l.chars().skip(1).collect::<String>().trim().to_owned() } else { l.clone() };
            Choice { index: i + 1, label, selected, hotkey: None }
        })
        .collect();
    // The question is somewhere in the paragraph above: the nearest line with a '?' (the terminal may have wrapped
    // it anywhere), else the nearest line of text.
    let above: Vec<&String> = lines[..lo].iter().rev().filter(|l| !l.is_empty()).take(8).collect();
    let q_at = above.iter().position(|l| l.contains('?')).unwrap_or(0);
    let question = above.get(q_at).map(|s| s.to_string()).unwrap_or_default();
    let mut context: Vec<String> =
        above.iter().enumerate().filter(|(i, _)| *i != q_at).take(MAX_CONTEXT).map(|(_, l)| l.to_string()).collect();
    context.reverse();
    Some(Prompt { kind: PromptKind::Menu, id: prompt_id(&question, &choices), question, context, choices })
}

pub fn analyze(screen: &str) -> Analysis {
    let all: Vec<String> = screen.lines().map(clean).filter(|l| !l.is_empty()).collect();
    let lines: Vec<String> = all[all.len().saturating_sub(WINDOW)..].to_vec();
    let tail: Vec<String> = lines[lines.len().saturating_sub(6)..].to_vec();

    let prompt = find_menu(&lines).or_else(|| find_yes_no(&lines)).or_else(|| find_plain_menu(screen));
    let bottom = &lines[lines.len().saturating_sub(10)..];
    let working = bottom.iter().any(|l| {
        let l = l.to_ascii_lowercase();
        l.contains("esc to interrupt") || l.contains("ctrl+c to interrupt") || l.contains("ctrl-c to interrupt")
    });

    let activity = if prompt.is_some() {
        Activity::Waiting
    } else if working {
        Activity::Working
    } else if lines.iter().rev().take(6).any(|l| l == ">" || l.starts_with("> ") || l.starts_with("› ") || l == "›") {
        Activity::Idle
    } else {
        Activity::Unknown
    };
    Analysis { activity, prompt, tail }
}

/// Which choice means "yes"? First option that reads like approval, else the first.
pub fn approve_index(p: &Prompt) -> usize {
    const YES: &[&str] = &["yes", "allow", "approve", "proceed", "continue", "ok", "accept", "run"];
    p.choices
        .iter()
        .find(|c| {
            let l = c.label.to_ascii_lowercase();
            YES.iter().any(|w| l.starts_with(w))
        })
        .or(p.choices.first())
        .map_or(1, |c| c.index)
}

/// Which choice means "no"? First option that reads like refusal, else the last.
pub fn deny_index(p: &Prompt) -> usize {
    const NO: &[&str] = &["no", "deny", "reject", "cancel", "decline", "skip"];
    p.choices
        .iter()
        .find(|c| {
            let l = c.label.to_ascii_lowercase();
            NO.iter().any(|w| l == *w || l.starts_with(&format!("{w} ")) || l.starts_with(&format!("{w},")))
        })
        .or(p.choices.last())
        .map_or(1, |c| c.index)
}

/// Keystrokes that select choice number `index` (1-based) in `p`.
pub fn steps_for(p: &Prompt, index: usize) -> Option<Vec<Step>> {
    let target = p.choices.iter().find(|c| c.index == index)?;
    if p.kind == PromptKind::YesNo {
        let key = target.hotkey.clone()?;
        return Some(vec![Step::Text(key), Step::Key("Enter")]);
    }
    // An advertised "esc" hotkey is cancel: just press Escape.
    if target.hotkey.as_deref() == Some("esc") {
        return Some(vec![Step::Key("Escape")]);
    }
    // With a visible cursor, walking to the option and pressing Enter works in any selector.
    if let Some(cur) = p.choices.iter().find(|c| c.selected) {
        let mut steps = Vec::new();
        let (from, to) = (cur.index, target.index);
        let key = if to > from { "Down" } else { "Up" };
        steps.extend(std::iter::repeat_n(Step::Key(key), from.abs_diff(to)));
        steps.push(Step::Key("Enter"));
        return Some(steps);
    }
    // No cursor visible: fall back to the option's number, then confirm.
    Some(vec![Step::Text(target.index.to_string()), Step::Key("Enter")])
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_PERMISSION: &str = "\
 ● Bash(rm -rf build)

╭──────────────────────────────────────────────────────────────╮
│ Bash command                                                 │
│                                                              │
│   rm -rf build                                               │
│   Remove build directory                                     │
│                                                              │
│ Do you want to proceed?                                      │
│ ❯ 1. Yes                                                     │
│   2. Yes, and don't ask again for rm commands in /tmp        │
│   3. No, and tell Claude what to do differently (esc)        │
╰──────────────────────────────────────────────────────────────╯
";

    const CODEX_APPROVAL: &str = "\
  Would you like to run the following command?

  $ rm -rf build

› 1. Yes, proceed (y)
  2. Yes, and don't ask again for this command (a)
  3. No, and tell Codex what to do differently (esc)

  Press enter to confirm or esc to cancel
";

    #[test]
    fn claude_permission_menu() {
        let a = analyze(CLAUDE_PERMISSION);
        assert_eq!(a.activity, Activity::Waiting);
        let p = a.prompt.expect("prompt");
        assert_eq!(p.kind, PromptKind::Menu);
        assert_eq!(p.question, "Do you want to proceed?");
        assert_eq!(p.choices.len(), 3);
        assert!(p.choices[0].selected && !p.choices[1].selected);
        assert_eq!(p.choices[2].hotkey.as_deref(), Some("esc"));
        assert!(p.context.iter().any(|l| l.contains("rm -rf build")));
        assert_eq!(approve_index(&p), 1);
        assert_eq!(deny_index(&p), 3);
    }

    #[test]
    fn codex_approval_menu() {
        let p = analyze(CODEX_APPROVAL).prompt.expect("prompt");
        assert_eq!(p.question, "Would you like to run the following command?");
        assert!(p.context.iter().any(|l| l == "$ rm -rf build"));
        assert_eq!(p.choices[0].hotkey.as_deref(), Some("y"));
        assert_eq!(p.choices[1].hotkey.as_deref(), Some("a"));
        assert_eq!(approve_index(&p), 1);
        assert_eq!(deny_index(&p), 3);
    }

    const CLAUDE_TRUST: &str = "\
Accessing workspace:

/Users/sandeep

Quick safety check: Is this a project you
created or one you trust? (Like your own code,
a well-known open source project, or work from
your team). If not, take a moment to review
what's in this folder first.

Claude Code'll be able to read, edit, and
execute files here.

Security guide

❯ No, exit
  Yes, I trust this folder

Enter to confirm · Esc to cancel
";

    #[test]
    fn claude_trust_dialog_is_a_prompt() {
        let a = analyze(CLAUDE_TRUST);
        assert_eq!(a.activity, Activity::Waiting);
        let p = a.prompt.expect("the trust dialog is a question");
        assert_eq!(p.kind, PromptKind::Menu);
        let labels: Vec<&str> = p.choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["No, exit", "Yes, I trust this folder"]);
        assert!(p.choices[0].selected && !p.choices[1].selected);
        assert!(p.question.contains("trust"), "question was {:?}", p.question);
    }

    #[test]
    fn trust_dialog_yes_and_no_walk_the_cursor() {
        let p = analyze(CLAUDE_TRUST).prompt.unwrap();
        assert_eq!(approve_index(&p), 2);
        assert_eq!(deny_index(&p), 1);
        // The cursor starts on "No, exit": yes is one Down then Enter, no is just Enter.
        assert_eq!(steps_for(&p, 2), Some(vec![Step::Key("Down"), Step::Key("Enter")]));
        assert_eq!(steps_for(&p, 1), Some(vec![Step::Key("Enter")]));
    }

    #[test]
    fn trust_dialog_with_the_cursor_moved_down() {
        let moved = CLAUDE_TRUST.replace("❯ No, exit\n  Yes, I trust", "  No, exit\n❯ Yes, I trust");
        let p = analyze(&moved).prompt.unwrap();
        assert!(!p.choices[0].selected && p.choices[1].selected);
        assert_eq!(steps_for(&p, 1), Some(vec![Step::Key("Up"), Step::Key("Enter")]));
        assert_eq!(steps_for(&p, 2), Some(vec![Step::Key("Enter")]));
    }

    #[test]
    fn a_shell_prompt_is_not_a_plain_menu() {
        let shell = "~/code\n❯ git status\nOn branch main\n\n~/code\n❯ ";
        assert!(analyze(shell).prompt.is_none());
    }

    #[test]
    fn a_list_without_the_confirm_footer_is_not_a_plain_menu() {
        let list = CLAUDE_TRUST.replace("Enter to confirm · Esc to cancel", "↑↓ to navigate · Esc to close");
        assert!(analyze(&list).prompt.is_none());
    }

    #[test]
    fn a_lone_option_or_two_cursors_are_not_a_menu() {
        let one = CLAUDE_TRUST.replace("  Yes, I trust this folder\n", "");
        assert!(analyze(&one).prompt.is_none());
        let two = CLAUDE_TRUST.replace("  Yes, I trust", "❯ Yes, I trust");
        assert!(analyze(&two).prompt.is_none());
    }

    #[test]
    fn an_answered_trust_dialog_is_gone() {
        let after = format!("{CLAUDE_TRUST}\nWelcome to Claude Code\n> ");
        assert!(analyze(&after).prompt.is_none());
    }

    #[test]
    fn keystrokes_walk_the_selector() {
        let p = analyze(CLAUDE_PERMISSION).prompt.unwrap();
        // Cursor is on 1.
        assert_eq!(steps_for(&p, 1).unwrap(), vec![Step::Key("Enter")]);
        assert_eq!(steps_for(&p, 2).unwrap(), vec![Step::Key("Down"), Step::Key("Enter")]);
        // 3 advertises (esc): just cancel.
        assert_eq!(steps_for(&p, 3).unwrap(), vec![Step::Key("Escape")]);
        assert!(steps_for(&p, 9).is_none());
    }

    #[test]
    fn moving_up_from_a_lower_cursor() {
        let screen = "Pick one?\n  1. alpha\n  2. beta\n❯ 3. gamma\n";
        let p = analyze(screen).prompt.unwrap();
        assert_eq!(
            steps_for(&p, 1).unwrap(),
            vec![Step::Key("Up"), Step::Key("Up"), Step::Key("Enter")]
        );
    }

    #[test]
    fn no_cursor_falls_back_to_digit() {
        let screen = "Do you want to continue?\n 1. Yes\n 2. No\n";
        let p = analyze(screen).prompt.unwrap();
        assert_eq!(steps_for(&p, 2).unwrap(), vec![Step::Text("2".into()), Step::Key("Enter")]);
    }

    #[test]
    fn inline_yes_no() {
        let a = analyze("Compiling...\nOverwrite existing file? [y/N] ");
        let p = a.prompt.expect("prompt");
        assert_eq!(p.kind, PromptKind::YesNo);
        assert_eq!(p.question, "Overwrite existing file? [y/N]");
        assert_eq!(steps_for(&p, 1).unwrap(), vec![Step::Text("y".into()), Step::Key("Enter")]);
        assert_eq!(steps_for(&p, deny_index(&p)).unwrap()[0], Step::Text("n".into()));
    }

    #[test]
    fn answered_menu_is_no_longer_a_prompt() {
        let answered = "Do you want to proceed?\n❯ 1. Yes\n  2. No\nGOT: 1\nnext command output\n";
        assert!(analyze(answered).prompt.is_none());
        // A footer hint under a live menu is fine; a typed answer line is not.
        let live = "Do you want to proceed?\n❯ 1. Yes\n  2. No\n\n  Esc to cancel · Tab to add instructions\n";
        assert!(analyze(live).prompt.is_some());
    }

    #[test]
    fn answered_yes_no_is_no_longer_a_prompt() {
        assert!(analyze("Overwrite? [y/N] y\n").prompt.is_none());
        assert!(analyze("Overwrite? [y/N] y\nDone.\n").prompt.is_none());
        assert!(analyze("Overwrite? [y/N]: ").prompt.is_some());
        // Still waiting with an unrelated line after it? No: that line would be the answer/output.
        assert!(analyze("Continue? (yes/no)\n").prompt.is_some());
    }

    #[test]
    fn plain_numbered_list_is_not_a_prompt() {
        let screen = "Here is the plan:\n1. Read the file\n2. Edit it\n3. Run the tests\n";
        assert!(analyze(screen).prompt.is_none());
    }

    #[test]
    fn old_menu_scrolled_off_is_ignored() {
        let mut screen = String::from("Do you want to proceed?\n❯ 1. Yes\n  2. No\n");
        for i in 0..30 {
            screen.push_str(&format!("output line {i}\n"));
        }
        assert!(analyze(&screen).prompt.is_none());
    }

    #[test]
    fn working_idle_unknown() {
        let working = analyze("✻ Thinking… (12s · esc to interrupt)\n");
        assert_eq!(working.activity, Activity::Working);
        let codex = analyze("• Working (3s • esc to interrupt)\n");
        assert_eq!(codex.activity, Activity::Working);
        let idle = analyze("Done. Anything else?\n│ > │\n");
        assert_eq!(idle.activity, Activity::Idle);
        assert_eq!(analyze("\n\n").activity, Activity::Unknown);
        assert_eq!(analyze("$ ls\nfoo bar\n").activity, Activity::Unknown);
    }

    #[test]
    fn prompt_id_tracks_content() {
        let a = analyze(CLAUDE_PERMISSION).prompt.unwrap();
        let b = analyze(CLAUDE_PERMISSION).prompt.unwrap();
        let c = analyze(&CLAUDE_PERMISSION.replace("proceed", "continue")).prompt.unwrap();
        assert_eq!(a.id, b.id);
        assert_ne!(a.id, c.id);
    }
}
