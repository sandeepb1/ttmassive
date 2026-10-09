//! The suggested reply Claude Code shows, greyed out, in an empty input box.
//!
//! It is drawn with the "faint" attribute right after the `❯` prompt, which plain screen text
//! can't tell apart from something you typed, so this reads a capture that keeps the escape codes.
//! An empty Enter in the terminal would accept it; the chat offers it as a chip instead.

/// The faint text after the prompt on the input line, if there is any.
pub fn faint_input(styled_screen: &str) -> Option<String> {
    let line = styled_screen
        .lines()
        .rev()
        .take(14)
        .find(|l| strip_escapes(l).trim_start().starts_with('❯'))?;
    // Everything after the glyph, escape codes kept.
    let rest = line.split_once('❯')?.1;
    // Typed text has no faint attribute; the suggestion is faint from its first character on.
    let start = rest.find("\x1b[2m")?;
    if !strip_escapes(&rest[..start]).trim().is_empty() {
        return None;
    }
    let text = &rest[start + "\x1b[2m".len()..];
    let end = text.find("\x1b[").unwrap_or(text.len());
    let text = text[..end].trim();
    // Claude's own idle hint on a fresh conversation is not a suggestion.
    (!text.is_empty() && !text.starts_with("Try \"")).then(|| text.to_owned())
}

/// `s` without ANSI escape sequences (CSI only: all tmux emits for text).
fn strip_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'[') {
            it.next();
            for n in it.by_ref() {
                if ('@'..='~').contains(&n) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = "────────────────────────────────────────";

    fn screen(input: &str) -> String {
        format!("\x1b[2m{RULE}\x1b[0m\n{input}\n\x1b[2m{RULE}\x1b[0m\n  \x1b[2m5h 15% (4h29m)\x1b[0m\n  ⏵⏵ auto mode on\n")
    }

    #[test]
    fn reads_the_faint_suggestion() {
        let s = screen("\x1b[39m❯ \x1b[2mgo ahead with step 2\x1b[0m");
        assert_eq!(faint_input(&s).as_deref(), Some("go ahead with step 2"));
    }

    #[test]
    fn typed_text_is_not_a_suggestion() {
        assert_eq!(faint_input(&screen("\x1b[39m❯ hello there")), None);
        // Typed text followed by something faint is still typed.
        assert_eq!(faint_input(&screen("\x1b[39m❯ hello \x1b[2mthere\x1b[0m")), None);
    }

    #[test]
    fn an_empty_prompt_has_none() {
        assert_eq!(faint_input(&screen("\x1b[39m❯ ")), None);
    }

    #[test]
    fn the_fresh_conversation_hint_is_ignored() {
        let s = screen("\x1b[39m❯ \x1b[2mTry \"how does <filepath> work?\"\x1b[0m");
        assert_eq!(faint_input(&s), None);
    }

    #[test]
    fn a_selected_menu_row_is_not_the_input() {
        // A menu cursor also uses ❯, but its row is not faint after the glyph.
        assert_eq!(faint_input("\x1b[36m❯ 1. Yes\x1b[39m\n  2. No\n"), None);
    }
}
