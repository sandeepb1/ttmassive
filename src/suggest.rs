//! Suggestions for the chat composer: slash commands when you type `/`, files when you type `@`.
//!
//! Slash commands are the agent's built-in ones plus whatever you have defined as commands or
//! skills in your own folders. Plugin-provided commands live in plugin folders ttm does not read,
//! so they are not suggested (you can still type them). Files come from the session's working
//! directory, skipping anything `.gitignore` excludes and hidden files, and are only ever
//! returned as paths relative to it.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{agentlog, config::Agent};

const MAX_RESULTS: usize = 12;
/// How many files are read from the directory before ranking: a huge tree must not stall a keystroke.
const MAX_FILES: usize = 5000;
const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    /// What the list shows.
    pub label: String,
    /// What replaces the typed word when you pick it.
    pub insert: String,
    pub detail: String,
}

const CLAUDE_COMMANDS: &[(&str, &str)] = &[
    ("add-dir", "Add a working directory"),
    ("agents", "Manage sub-agents"),
    ("clear", "Start a new conversation"),
    ("compact", "Summarise the conversation to free up context"),
    ("config", "Open settings"),
    ("context", "Show what is using the context window"),
    ("copy", "Copy the last reply"),
    ("cost", "Show token usage and cost"),
    ("doctor", "Check the installation"),
    ("effort", "Set how hard the model thinks"),
    ("exit", "Quit"),
    ("export", "Export the conversation"),
    ("help", "Show help"),
    ("hooks", "Manage hooks"),
    ("init", "Create a CLAUDE.md for this project"),
    ("login", "Sign in"),
    ("logout", "Sign out"),
    ("mcp", "Manage MCP servers"),
    ("memory", "Edit memory files"),
    ("model", "Switch model"),
    ("permissions", "Manage tool permissions"),
    ("plugin", "Manage plugins"),
    ("resume", "Resume an earlier conversation"),
    ("review", "Review a pull request"),
    ("rewind", "Go back to an earlier point"),
    ("status", "Show account and model status"),
    ("terminal-setup", "Set up terminal keys"),
    ("theme", "Change the colour theme"),
    ("usage", "Show plan usage"),
    ("vim", "Toggle vim editing"),
];

const CODEX_COMMANDS: &[(&str, &str)] = &[
    ("compact", "Summarise the conversation to free up context"),
    ("diff", "Show the git diff"),
    ("init", "Create an AGENTS.md for this project"),
    ("logout", "Sign out"),
    ("mcp", "List MCP tools"),
    ("mention", "Mention a file"),
    ("model", "Choose the model and reasoning effort"),
    ("new", "Start a new conversation"),
    ("permissions", "Choose what Codex is allowed to do"),
    ("quit", "Quit"),
    ("resume", "Resume an earlier conversation"),
    ("review", "Review changes and find issues"),
    ("status", "Show the current session configuration"),
];

/// `description:` from a Markdown file's front matter, else its first line of text.
fn description(text: &str) -> String {
    let body = text.strip_prefix("---").and_then(|r| r.split_once("\n---"));
    if let Some((front, _)) = body {
        for line in front.lines() {
            if let Some(d) = line.strip_prefix("description:") {
                return d.trim().trim_matches(['"', '\'']).to_owned();
            }
        }
    }
    let after = body.map_or(text, |(_, rest)| rest);
    after
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty() && *l != "---")
        .unwrap_or("")
        .chars()
        .take(90)
        .collect()
}

/// Commands from `*.md` files in `dir` (the file name is the command).
fn commands_in(dir: &Path, out: &mut Vec<(String, String)>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "md") {
            continue;
        }
        let (Some(name), Ok(text)) = (path.file_stem().and_then(|n| n.to_str()), std::fs::read_to_string(&path)) else {
            continue;
        };
        out.push((name.to_owned(), description(&text)));
    }
}

/// Skills: `<dir>/<name>/SKILL.md`.
fn skills_in(dir: &Path, out: &mut Vec<(String, String)>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        let (Some(name), Ok(text)) = (path.file_name().and_then(|n| n.to_str()), std::fs::read_to_string(path.join("SKILL.md")))
        else {
            continue;
        };
        out.push((name.to_owned(), description(&text)));
    }
}

/// Slash commands matching `query` (what follows the `/`). Blocking: reads directories.
pub fn slash(agent: Agent, project: Option<&Path>, query: &str) -> Vec<Suggestion> {
    let builtin = match agent {
        Agent::Claude => CLAUDE_COMMANDS,
        Agent::Codex => CODEX_COMMANDS,
        Agent::Other => &[],
    };
    let mut all: Vec<(String, String)> = Vec::new();
    // Later entries win, so a project's own command overrides a user one, which overrides a built-in.
    all.extend(builtin.iter().map(|(n, d)| ((*n).to_owned(), (*d).to_owned())));
    match agent {
        Agent::Claude => {
            if let Some(dir) = agentlog::claude_dir() {
                commands_in(&dir.join("commands"), &mut all);
                skills_in(&dir.join("skills"), &mut all);
            }
            if let Some(p) = project {
                commands_in(&p.join(".claude/commands"), &mut all);
                skills_in(&p.join(".claude/skills"), &mut all);
            }
        }
        Agent::Codex => {
            if let Some(dir) = agentlog::codex_dir() {
                commands_in(&dir.join("prompts"), &mut all);
            }
        }
        Agent::Other => {}
    }
    rank_commands(all, query)
}

fn rank_commands(all: Vec<(String, String)>, query: &str) -> Vec<Suggestion> {
    let q = query.to_lowercase();
    let mut seen = std::collections::HashMap::new();
    for (name, desc) in all {
        seen.insert(name, desc); // the last definition of a name wins
    }
    let mut hits: Vec<(u8, String, String)> = seen
        .into_iter()
        .filter_map(|(name, desc)| {
            let lower = name.to_lowercase();
            let rank = if lower.starts_with(&q) {
                0
            } else if lower.contains(&q) {
                1
            } else {
                return None;
            };
            Some((rank, name, desc))
        })
        .collect();
    hits.sort();
    hits.into_iter()
        .take(MAX_RESULTS)
        .map(|(_, name, detail)| Suggestion { label: format!("/{name}"), insert: format!("/{name} "), detail })
        .collect()
}

/// How well `path` matches `query`: `None` unless every query letter appears in order.
/// Higher is better; matches in the file name and on word boundaries count for more.
fn score(query: &str, path: &str) -> Option<i64> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let p: Vec<char> = path.to_lowercase().chars().collect();
    let base = p.iter().rposition(|&c| c == '/').map_or(0, |i| i + 1);
    let mut qi = 0;
    let mut score = 0i64;
    let mut prev: Option<usize> = None;
    for (i, &c) in p.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            score += 1;
            if prev.is_some_and(|pv| pv + 1 == i) {
                score += 8; // consecutive letters
            }
            if i == 0 || matches!(p[i - 1], '/' | '.' | '_' | '-') {
                score += 6; // the start of a word
            }
            if i == base {
                score += 10; // the start of the file name
            }
            prev = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    let name: String = p[base..].iter().collect();
    if !q.is_empty() && name.contains(&q.iter().collect::<String>()) {
        score += 20;
    }
    Some(score * 4 - p.len() as i64)
}

/// Files under `dir` matching `query`, as paths relative to it. Blocking: walks the directory.
pub fn files(dir: &Path, query: &str) -> Vec<Suggestion> {
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(MAX_DEPTH))
        .follow_links(false)
        .require_git(false) // honour .gitignore even when the folder is not a repository
        .build();
    let mut found: Vec<String> = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(dir) else { continue };
        if let Some(rel) = rel.to_str() {
            found.push(rel.to_owned());
        }
        if found.len() >= MAX_FILES {
            break;
        }
    }
    let mut ranked: Vec<(i64, String)> = found.into_iter().filter_map(|p| score(query, &p).map(|s| (s, p))).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    ranked
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, rel)| {
            let detail = Path::new(&rel).parent().and_then(|p| p.to_str()).unwrap_or("").to_owned();
            let label = Path::new(&rel).file_name().and_then(|n| n.to_str()).unwrap_or(&rel).to_owned();
            Suggestion { label, insert: format!("@{rel} "), detail }
        })
        .collect()
}

/// The directory a session works in, if it still exists.
pub fn project_dir(dir: Option<&PathBuf>) -> Option<PathBuf> {
    dir.filter(|d| d.is_dir()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_beat_scattered_letters() {
        let main = score("main", "src/main.rs").unwrap();
        let domain = score("main", "src/domain.rs").unwrap();
        assert!(main > domain, "{main} vs {domain}");
        assert_eq!(score("zzz", "src/main.rs"), None);
        // Letters must come in order.
        assert!(score("sm", "src/main.rs").is_some());
        assert!(score("ms", "main.rs").is_some());
        assert_eq!(score("rsm", "src/main"), None);
    }

    #[test]
    fn a_shorter_path_wins_a_tie_and_case_does_not_matter() {
        assert!(score("api", "src/api.rs").unwrap() > score("api", "web/src/deep/api.rs").unwrap());
        assert!(score("README", "readme.md").is_some());
    }

    #[test]
    fn commands_are_filtered_ranked_and_deduplicated() {
        let all = vec![
            ("compact".to_owned(), "built-in".to_owned()),
            ("commit".to_owned(), "mine".to_owned()),
            ("review".to_owned(), "built-in".to_owned()),
            ("pr-review".to_owned(), "mine".to_owned()),
            ("review".to_owned(), "project override".to_owned()),
        ];
        let got = rank_commands(all.clone(), "rev");
        assert_eq!(got.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["/review", "/pr-review"]);
        assert_eq!(got[0].detail, "project override");
        assert_eq!(got[0].insert, "/review ");
        let by_prefix = rank_commands(all, "co");
        assert_eq!(by_prefix.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["/commit", "/compact"]);
        assert!(rank_commands(vec![], "x").is_empty());
    }

    #[test]
    fn descriptions_come_from_front_matter_or_the_first_line() {
        assert_eq!(description("---\nname: x\ndescription: \"Does the thing\"\n---\nBody"), "Does the thing");
        assert_eq!(description("---\nname: x\n---\n# Title line\nmore"), "Title line");
        assert_eq!(description("\n\nJust text here"), "Just text here");
        assert_eq!(description(""), "");
    }

    #[test]
    fn built_in_lists_are_sorted_and_have_no_duplicates() {
        for list in [CLAUDE_COMMANDS, CODEX_COMMANDS] {
            let names: Vec<&str> = list.iter().map(|(n, _)| *n).collect();
            let mut sorted = names.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(names, sorted);
        }
    }

    #[test]
    fn file_search_skips_ignored_and_hidden_files_and_returns_relative_paths() {
        let dir = std::env::temp_dir().join(format!("ttm-suggest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".secret")).unwrap();
        for f in ["a.txt", "sub/b.rs", "noise.log", ".hidden", ".secret/key"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        std::fs::write(dir.join(".gitignore"), "*.log\n").unwrap();
        let all = files(&dir, "");
        let inserts: Vec<&str> = all.iter().map(|s| s.insert.as_str()).collect();
        assert!(inserts.contains(&"@a.txt "));
        assert!(inserts.contains(&"@sub/b.rs "));
        assert!(!inserts.iter().any(|i| i.contains("noise.log")), "{inserts:?}");
        assert!(!inserts.iter().any(|i| i.contains(".hidden") || i.contains(".secret")), "{inserts:?}");
        assert!(inserts.iter().all(|i| !i.starts_with("@/") && !i.contains("..")));
        let hit = files(&dir, "b.rs");
        assert_eq!((hit[0].label.as_str(), hit[0].detail.as_str()), ("b.rs", "sub"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
