//! The XML-like wrappers Claude Code and Codex put around parts of a message: a slash command and
//! its output, a `!` shell command, a finished background task, pasted text, injected context.
//! They are in the log as plain text, so without this they show up in the chat as raw `<bash-input>`.
//!
//! Only the tags listed here are given meaning; any other `<tag>` (HTML in a question, a generic
//! type, XML a user pasted) stays ordinary text.

/// A tag whose content is not for the reader: context the agent was given, not something said.
const DROPPED: &[&str] = &[
    "system-reminder",
    "local-command-caveat",
    "command-message",
    "user-prompt-submit-hook",
    "new-diagnostics",
    "ide_opened_file",
    "ide_selection",
    // Codex's per-turn context.
    "environment_context",
    "user_instructions",
    "permissions",
    "collaboration_mode",
    "multi_agent_mode",
    "multi_agent_role",
    "skills_instructions",
    "apps_instructions",
    "plugins_instructions",
    "recommended_plugins",
    "model_switch",
];

/// A wrapper around text that should be read as if it were not there.
const UNWRAPPED: &[&str] = &["task", "proposed_plan", "send_user_message_question_reply", "tool_use_error", "persisted-output"];

/// Tags that mean something of their own; see `Parsed`.
const MEANINGFUL: &[&str] = &[
    "command-name",
    "command-args",
    "local-command-stdout",
    "local-command-stderr",
    "bash-input",
    "bash-stdout",
    "bash-stderr",
    "task-notification",
    "pasted_content",
    "turn_aborted",
];

fn known(name: &str) -> bool {
    DROPPED.contains(&name) || UNWRAPPED.contains(&name) || MEANINGFUL.contains(&name)
}

/// One tag with its attributes (as written) and what is between its opening and closing.
#[derive(Debug, PartialEq, Eq)]
pub struct Tag<'a> {
    pub name: &'a str,
    pub attrs: &'a str,
    pub body: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Tag(Tag<'a>),
}

fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// `<name>` or `<name attr="x">` at the start of `s`: the name, the attributes, and the length.
fn open_tag(s: &str) -> Option<(&str, &str, usize)> {
    let rest = s.strip_prefix('<')?;
    let name_len = rest.find(|c: char| !name_char(c))?;
    if name_len == 0 || !rest.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    let name = &rest[..name_len];
    let after = &rest[name_len..];
    let end = after.find('>')?;
    let attrs = &after[..end];
    // Attributes are separated from the name by a space, and a tag does not run on for ever.
    if (!attrs.is_empty() && !attrs.starts_with(char::is_whitespace)) || attrs.contains('<') || attrs.len() > 200 {
        return None;
    }
    Some((name, attrs.trim(), 1 + name_len + end + 1))
}

/// Where the closing tag of `name` starts and how long it is. Claude writes the closing tag of
/// pasted text with the attribute repeated (`</pasted_content id="6e14">`), so that counts too.
fn close_tag(s: &str, name: &str) -> Option<(usize, usize)> {
    let needle = format!("</{name}");
    let mut from = 0;
    while let Some(i) = s[from..].find(&needle) {
        let at = from + i;
        let after = &s[at + needle.len()..];
        if after.starts_with('>') {
            return Some((at, needle.len() + 1));
        }
        if after.starts_with(char::is_whitespace)
            && let Some(end) = after.find('>')
            && !after[..end].contains('<')
        {
            return Some((at, needle.len() + end + 1));
        }
        from = at + needle.len();
    }
    None
}

/// `s` as text and known tags, in order. A dropped tag that is never closed takes the rest of
/// the text with it, as Claude's reminders do when a message was cut short.
pub fn split(s: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut text_from = 0;
    let mut i = 0;
    while let Some(off) = s[i..].find('<') {
        let at = i + off;
        let Some((name, attrs, open_len)) = open_tag(&s[at..]).filter(|(n, _, _)| known(n)) else {
            i = at + 1;
            continue;
        };
        let body_from = at + open_len;
        match close_tag(&s[body_from..], name) {
            Some((rel, close_len)) => {
                if at > text_from {
                    out.push(Piece::Text(&s[text_from..at]));
                }
                out.push(Piece::Tag(Tag { name, attrs, body: &s[body_from..body_from + rel] }));
                text_from = body_from + rel + close_len;
                i = text_from;
            }
            None if DROPPED.contains(&name) => {
                if at > text_from {
                    out.push(Piece::Text(&s[text_from..at]));
                }
                return out;
            }
            None => i = at + 1,
        }
    }
    if text_from < s.len() {
        out.push(Piece::Text(&s[text_from..]));
    }
    out
}

/// The value of attribute `key` in `attrs` (`id="6e14"`).
pub fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let rest = &attrs[attrs.find(&format!("{key}="))? + key.len() + 1..];
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let rest = &rest[1..];
    Some(&rest[..rest.find(quote)?])
}

/// The text of the first `<name>...</name>` in `s`, whatever is around it.
fn inner<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = s.find(&open)? + open.len();
    let (end, _) = close_tag(&s[start..], name)?;
    Some(s[start..start + end].trim())
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Paste {
    pub id: Option<String>,
    pub text: String,
}

/// A finished background task (`<task-notification>`).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Task {
    pub status: Option<String>,
    pub summary: Option<String>,
    pub result: Option<String>,
}

impl Task {
    /// One line for the chat: what the task reported, else its status.
    pub fn line(&self) -> String {
        match (&self.summary, &self.status) {
            (Some(s), _) if !s.is_empty() => s.clone(),
            (_, Some(st)) if !st.is_empty() => format!("Background task {st}"),
            _ => "Background task finished".to_owned(),
        }
    }
}

/// What a message turns out to say once its wrappers are read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// What is left of the message outside the wrappers: what was actually said.
    pub text: String,
    /// A slash command: its name and arguments.
    pub command: Option<(String, String)>,
    /// What a slash command printed (`<local-command-stdout>` and `-stderr`).
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    /// A `!` command you ran, and what it printed.
    pub bash_input: Option<String>,
    pub bash_stdout: Option<String>,
    pub bash_stderr: Option<String>,
    pub task: Option<Task>,
    pub pastes: Vec<Paste>,
    /// Codex says the turn was interrupted.
    pub aborted: bool,
}

pub fn parse(s: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut text = String::new();
    let (mut name, mut args) = (None, None);
    for piece in split(s) {
        match piece {
            Piece::Text(t) => text.push_str(t),
            Piece::Tag(Tag { name: n, attrs, body }) => match n {
                "command-name" => name = Some(body.trim().to_owned()),
                "command-args" => args = Some(body.trim().to_owned()),
                "local-command-stdout" => p.stdout = Some(body.to_owned()),
                "local-command-stderr" => p.stderr = Some(body.to_owned()),
                "bash-input" => p.bash_input = Some(body.trim().to_owned()),
                "bash-stdout" => p.bash_stdout = Some(body.to_owned()),
                "bash-stderr" => p.bash_stderr = Some(body.to_owned()),
                "task-notification" => {
                    p.task = Some(Task {
                        status: inner(body, "status").map(str::to_owned),
                        summary: inner(body, "summary").map(str::to_owned),
                        result: inner(body, "result").map(str::to_owned),
                    })
                }
                "pasted_content" => p.pastes.push(Paste { id: attr(attrs, "id").map(str::to_owned), text: body.trim_matches('\n').to_owned() }),
                "turn_aborted" => p.aborted = true,
                n if UNWRAPPED.contains(&n) => text.push_str(body),
                _ => {} // dropped
            },
        }
    }
    p.command = name.map(|n| (n, args.unwrap_or_default()));
    p.text = text.trim().to_owned();
    p
}

/// A tool's output without what Claude wraps around it: reminders it appends at the end, and the
/// error or "output saved" wrapper that frames the whole of it. Tags in the middle are left alone,
/// since the output may be a file that happens to contain them.
pub fn clean_output(s: &str) -> String {
    let mut t = s.trim();
    while let Some(head) = t.strip_suffix("</system-reminder>").and_then(|r| r.rfind("<system-reminder>").map(|i| &r[..i])) {
        t = head.trim_end();
    }
    for name in ["tool_use_error", "persisted-output"] {
        if let Some(body) = t.strip_prefix(&format!("<{name}>")).and_then(|r| r.strip_suffix(&format!("</{name}>"))) {
            return body.trim().to_owned();
        }
    }
    t.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_and_unknown_tags_are_left_alone() {
        assert_eq!(split("hello <b>there</b> and Vec<String>"), vec![Piece::Text("hello <b>there</b> and Vec<String>")]);
        let p = parse("what does <question> mean? 1 < 2 and <div class=\"x\">y</div>");
        assert_eq!(p.text, "what does <question> mean? 1 < 2 and <div class=\"x\">y</div>");
    }

    #[test]
    fn a_shell_command_and_its_output() {
        let p = parse("<bash-input> mkdir -p ~/x && ls </bash-input>");
        assert_eq!(p.bash_input.as_deref(), Some("mkdir -p ~/x && ls"));
        assert_eq!(p.text, "");

        let p = parse("<bash-stdout>a\nb</bash-stdout><bash-stderr>warn</bash-stderr>");
        assert_eq!(p.bash_stdout.as_deref(), Some("a\nb"));
        assert_eq!(p.bash_stderr.as_deref(), Some("warn"));

        let p = parse("<bash-stdout></bash-stdout><bash-stderr></bash-stderr>");
        assert_eq!(p.bash_stdout.as_deref(), Some(""));
        assert_eq!(p.bash_stderr.as_deref(), Some(""));
    }

    #[test]
    fn slash_commands_keep_their_name_and_arguments() {
        let p = parse("<command-message>grill-me</command-message>\n<command-name>/grill-me</command-name>\n<command-args>the plan</command-args>");
        assert_eq!(p.command, Some(("/grill-me".into(), "the plan".into())));
        assert_eq!(p.text, "");
        let p = parse("<command-name>/clear</command-name>");
        assert_eq!(p.command, Some(("/clear".into(), String::new())));
    }

    #[test]
    fn local_command_output_and_the_caveat() {
        assert_eq!(parse("<local-command-stdout>Set model to X</local-command-stdout>").stdout.as_deref(), Some("Set model to X"));
        assert_eq!(parse("<local-command-stderr>nope</local-command-stderr>").stderr.as_deref(), Some("nope"));
        assert_eq!(parse("<local-command-caveat>Caveat: ignore</local-command-caveat>"), Parsed::default());
    }

    #[test]
    fn pasted_content_closes_with_its_attribute_repeated() {
        let p = parse("\n\n<pasted_content id=\"6e14\">\n~/code main*\n❯ ls\nfile\n</pasted_content id=\"6e14\">\n");
        assert_eq!(p.text, "");
        assert_eq!(p.pastes, vec![Paste { id: Some("6e14".into()), text: "~/code main*\n❯ ls\nfile".into() }]);
    }

    #[test]
    fn pasted_content_beside_what_you_typed() {
        let p = parse("why does this fail?\n<pasted_content id='a1'>boom</pasted_content>\nand this\n<pasted_content id=\"b2\">bang</pasted_content id=\"b2\">");
        assert_eq!(p.text, "why does this fail?\n\nand this");
        assert_eq!(p.pastes.len(), 2);
        assert_eq!(p.pastes[0].id.as_deref(), Some("a1"));
        assert_eq!(p.pastes[1].text, "bang");
    }

    #[test]
    fn a_background_task_report() {
        let p = parse(
            "<task-notification>\n<task-id>b4y</task-id>\n<tool-use-id>toolu_01</tool-use-id>\n<output-file>/tmp/x.output</output-file>\n<status>completed</status>\n<summary>Background command \"Find it\" completed (exit code 0)</summary>\n</task-notification>",
        );
        let t = p.task.unwrap();
        assert_eq!(t.status.as_deref(), Some("completed"));
        assert_eq!(t.line(), "Background command \"Find it\" completed (exit code 0)");
        assert_eq!(Task { status: Some("failed".into()), ..Default::default() }.line(), "Background task failed");
        assert_eq!(Task::default().line(), "Background task finished");
    }

    #[test]
    fn injected_context_is_dropped_but_what_surrounds_it_stays() {
        let p = parse("<system-reminder>secret context</system-reminder>real question");
        assert_eq!(p.text, "real question");
        assert_eq!(parse("<system-reminder>only a reminder</system-reminder>"), Parsed::default());
        assert_eq!(parse("a<ide_opened_file>The user opened x.rs</ide_opened_file>b").text, "ab");
        assert_eq!(parse("<environment_context><cwd>/x</cwd></environment_context>hi").text, "hi");
    }

    #[test]
    fn an_unclosed_reminder_takes_the_rest_with_it() {
        assert_eq!(parse("question<system-reminder>cut off").text, "question");
        // Other tags that never close are just text.
        assert_eq!(parse("<bash-input>no end").text, "<bash-input>no end");
    }

    #[test]
    fn wrappers_that_only_frame_text_are_unwrapped() {
        assert_eq!(parse("<task>\nRun a review.\n</task>").text, "Run a review.");
        assert_eq!(parse("<proposed_plan>\n# Plan\n- a\n</proposed_plan>").text, "# Plan\n- a");
    }

    #[test]
    fn tool_output_loses_only_what_claude_wrapped_around_it() {
        assert_eq!(clean_output("<tool_use_error>File not found</tool_use_error>"), "File not found");
        assert_eq!(
            clean_output("<persisted-output>Output too large. Saved to /x\n\nPreview</persisted-output>"),
            "Output too large. Saved to /x\n\nPreview"
        );
        assert_eq!(clean_output("result\n\n<system-reminder>be careful</system-reminder>"), "result");
        assert_eq!(clean_output("r<system-reminder>a</system-reminder>\n<system-reminder>b</system-reminder>\n"), "r");
        // A file that mentions the tags is shown as it is.
        let file = "let t = \"<system-reminder>x</system-reminder>\"; // <tool_use_error> too";
        assert_eq!(clean_output(file), file);
        assert_eq!(clean_output("  plain  "), "plain");
    }

    #[test]
    fn codex_says_a_turn_was_interrupted() {
        let p = parse("<turn_aborted>\nThe user interrupted the previous turn.\n</turn_aborted>");
        assert!(p.aborted);
        assert_eq!(p.text, "");
    }

    #[test]
    fn tags_in_odd_places_do_not_break_the_parser() {
        for s in ["<", "<>", "< bash-input>", "<bash-input", "</bash-input>", "<bash-input></bash-input", "<<bash-input>x</bash-input>", "<1abc>x</1abc>", "é<é>", ""] {
            let _ = parse(s);
        }
        assert_eq!(parse("<<bash-input>x</bash-input>").bash_input.as_deref(), Some("x"));
        assert_eq!(parse("<bash-inputx>y</bash-inputx>").bash_input, None, "a longer name is another tag");
    }

    #[test]
    fn attributes_are_read() {
        assert_eq!(attr("id=\"6e14\"", "id"), Some("6e14"));
        assert_eq!(attr("a='1' id='2'", "id"), Some("2"));
        assert_eq!(attr("id=6e14", "id"), None);
        assert_eq!(attr("", "id"), None);
    }
}
