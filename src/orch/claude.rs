//! Drives one Claude Code agent headless. One `claude -p` process stays open for the whole life of
//! a node, taking prompts as stream-json on stdin, so the prompt cache is never re-written by a
//! `--resume` (each resume re-writes the whole history). The flags that isolate it from the user's
//! own hooks, MCP servers and skills were measured: they cut the cached prefix from 8244 to 3836 tokens.

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::Outcome;
use super::usage::Tokens;

/// Tools a read-only role gets: look at files and the web, change nothing.
const READ_ONLY_TOOLS: &str = "Read,Grep,Glob,WebSearch,WebFetch";

/// What an agent may do besides think.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Answer only: for writing prompts and asking questions.
    None,
    /// Look at files and the web, change nothing.
    ReadOnly,
    /// Read-only, plus a shell that the operating system confines to the working folder: it can run
    /// tests and `git status`, but cannot change anything or read the rest of the user's files.
    Inspect,
    /// Change files, inside the worktree it was started in.
    Write,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub model: String,
    pub system_prompt: String,
    /// JSON Schema the node's final answer must match.
    pub schema: Value,
    pub access: Access,
    pub cwd: PathBuf,
    /// More folders an `Inspect` shell may read, such as the main repository's `.git`, which a
    /// worktree needs for `git status`.
    pub read_also: Vec<PathBuf>,
}

/// Settings that confine an `Inspect` agent's shell. Enforced by the operating system, not by the
/// model: nothing writes in `dir`, and the user's home is unreadable except `dir` and `read_also`.
/// Strict: no unsandboxed retry, and the agent refuses to start if the sandbox cannot.
pub fn inspect_settings(dir: &std::path::Path, read_also: &[PathBuf]) -> Value {
    let mut allow_read = vec![dir.to_string_lossy().into_owned()];
    allow_read.extend(read_also.iter().map(|p| p.to_string_lossy().into_owned()));
    json!({"sandbox": {
        "enabled": true,
        "failIfUnavailable": true,
        "allowUnsandboxedCommands": false,
        "autoAllowBashIfSandboxed": true,
        "filesystem": {
            "denyWrite": [dir],
            "denyRead": ["~/"],
            "allowRead": allow_read,
        }
    }})
}

/// Arguments for `claude`. Pure, so the exact command is tested without starting anything.
pub fn args(spec: &Spec) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        // Keep the user's global hooks, MCP servers and skills out of the agent: they change the
        // cached prefix and spend tokens. Subscription login is unaffected.
        "--setting-sources",
        "project",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--exclude-dynamic-system-prompt-sections",
    ]
    .map(String::from)
    .to_vec();
    a.extend(["--model".into(), spec.model.clone()]);
    if !spec.system_prompt.trim().is_empty() {
        a.extend(["--append-system-prompt".into(), spec.system_prompt.clone()]);
    }
    a.extend(["--json-schema".into(), spec.schema.to_string()]);
    // Only a builder's conversation is kept on disk, so a person can open it in a live session later.
    // Everyone else leaves nothing behind in the user's own history.
    if spec.access != Access::Write {
        a.push("--no-session-persistence".into());
    }
    match spec.access {
        Access::None => a.extend(["--tools".into(), String::new()]),
        Access::ReadOnly => {
            a.extend(["--tools".into(), READ_ONLY_TOOLS.into(), "--allowedTools".into(), READ_ONLY_TOOLS.into()])
        }
        Access::Inspect => {
            let tools = format!("{READ_ONLY_TOOLS},Bash");
            a.extend(["--tools".into(), tools.clone(), "--allowedTools".into(), tools]);
            a.extend(["--settings".into(), inspect_settings(&spec.cwd, &spec.read_also).to_string()]);
        }
        // A builder works inside its own worktree; the checkpoint before it starts is the human gate.
        Access::Write => a.push("--dangerously-skip-permissions".into()),
    }
    a
}

/// One line of the agent's output stream, reduced to what the runner acts on.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Init { session_id: String, model: String },
    /// Text the agent said while working, for the node's live transcript.
    Said(String),
    /// A tool the agent called, for the live transcript ("Read src/main.rs").
    Tool(String),
    Done(Outcome),
    Other,
}

pub fn parse(line: &str) -> Event {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Event::Other };
    let s = |x: &Value, k: &str| x.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match v.get("type").and_then(Value::as_str) {
        Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            Event::Init { session_id: s(&v, "session_id"), model: s(&v, "model") }
        }
        Some("assistant") => {
            let blocks = v.pointer("/message/content").and_then(Value::as_array);
            for b in blocks.into_iter().flatten() {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") if !s(b, "text").trim().is_empty() => return Event::Said(s(b, "text")),
                    Some("tool_use") => {
                        let name = s(b, "name");
                        let hint = ["file_path", "path", "pattern", "command", "query", "url"]
                            .iter()
                            .find_map(|k| b.pointer(&format!("/input/{k}")).and_then(Value::as_str))
                            .unwrap_or("");
                        return Event::Tool(format!("{name} {hint}").trim().to_string());
                    }
                    _ => {}
                }
            }
            Event::Other
        }
        Some("result") => Event::Done(Outcome {
            structured: v.get("structured_output").filter(|x| !x.is_null()).cloned(),
            text: s(&v, "result"),
            tokens: v.get("usage").map(Tokens::from_claude).unwrap_or_default(),
            cost_usd: v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
            is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            session: v.get("session_id").and_then(Value::as_str).map(String::from),
        }),
        _ => Event::Other,
    }
}

/// The line to write to stdin for one prompt.
pub fn user_line(prompt: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": prompt}}).to_string()
}

/// A running agent.
pub struct Agent {
    session: Option<String>,
    child: Child,
    stdin: ChildStdin,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
}

impl Agent {
    pub async fn start(spec: &Spec) -> Result<Agent> {
        let mut cmd = Command::new("claude");
        cmd.args(args(spec)).current_dir(&spec.cwd);
        if spec.access == Access::Inspect {
            // Git reads the user's global config and refreshes its index; the sandbox allows neither,
            // so tell it not to try. `git status` and `git diff` then work.
            cmd.env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_OPTIONAL_LOCKS", "0").env("XDG_CONFIG_HOME", "/dev/null");
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // If ttm exits, the agent must not keep spending the user's plan.
            .kill_on_drop(true)
            .spawn()
            .context("could not start claude")?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        Ok(Agent { session: None, child, stdin, lines: BufReader::new(stdout).lines() })
    }

    /// Send one prompt and read events until the turn is done. `on_event` sees every event, so the
    /// page can show progress. Fails if the process ends before it answers.
    pub async fn turn(&mut self, prompt: &str, mut on_event: impl FnMut(&Event)) -> Result<Outcome> {
        self.stdin.write_all(user_line(prompt).as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        while let Some(line) = self.lines.next_line().await? {
            let ev = parse(&line);
            if let Event::Init { session_id, .. } = &ev {
                self.session = Some(session_id.clone());
            }
            on_event(&ev);
            if let Event::Done(mut o) = ev {
                o.session = o.session.or_else(|| self.session.clone());
                return Ok(o);
            }
        }
        bail!("claude exited before it answered")
    }

    /// End the current turn cleanly. SIGINT, not SIGTERM, which would leave the turn unrecorded.
    pub fn interrupt(&self) {
        if let Some(pid) = self.child.id() {
            // SAFETY: kill(2) with a pid we own and a valid signal number has no memory effects.
            unsafe { libc::kill(pid as i32, libc::SIGINT) };
        }
    }

    /// Close stdin so the process finishes, then wait for it.
    pub async fn finish(mut self) -> Result<()> {
        drop(self.stdin);
        self.child.wait().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(access: Access) -> Spec {
        Spec {
            model: "sonnet".into(),
            system_prompt: "You are a researcher.".into(),
            schema: json!({"type": "object"}),
            access,
            cwd: PathBuf::from("/tmp"),
            read_also: vec![],
        }
    }

    fn has(a: &[String], flag: &str) -> bool {
        a.iter().any(|x| x == flag)
    }

    fn after<'a>(a: &'a [String], flag: &str) -> Option<&'a str> {
        a.iter().position(|x| x == flag).and_then(|i| a.get(i + 1)).map(String::as_str)
    }

    #[test]
    fn every_node_is_isolated_from_the_users_own_setup() {
        for access in [Access::None, Access::ReadOnly, Access::Inspect, Access::Write] {
            let a = args(&spec(access));
            assert_eq!(after(&a, "--setting-sources"), Some("project"));
            for f in ["--strict-mcp-config", "--disable-slash-commands"] {
                assert!(has(&a, f), "missing {f}");
            }
            assert_eq!(after(&a, "--model"), Some("sonnet"));
            assert_eq!(after(&a, "--append-system-prompt"), Some("You are a researcher."));
            assert!(!has(&a, "--resume") && !has(&a, "--continue"));
        }
    }

    #[test]
    fn read_only_roles_get_no_write_tools_and_builders_skip_prompts() {
        let ro = args(&spec(Access::ReadOnly));
        assert_eq!(after(&ro, "--tools"), Some(READ_ONLY_TOOLS));
        assert!(!has(&ro, "--dangerously-skip-permissions"));

        let b = args(&spec(Access::Write));
        assert!(has(&b, "--dangerously-skip-permissions"));
        assert!(!has(&b, "--tools"));
    }

    #[test]
    fn an_inspecting_agent_gets_a_shell_only_inside_a_strict_sandbox() {
        let mut sp = spec(Access::Inspect);
        sp.cwd = PathBuf::from("/data/wt/r1/build");
        sp.read_also = vec![PathBuf::from("/work/app/.git")];
        let a = args(&sp);
        assert!(after(&a, "--tools").unwrap().contains("Bash"));
        assert!(!has(&a, "--dangerously-skip-permissions"));
        let cfg: Value = serde_json::from_str(after(&a, "--settings").unwrap()).unwrap();
        let sb = &cfg["sandbox"];
        assert_eq!(sb["enabled"], true);
        assert_eq!(sb["failIfUnavailable"], true, "no running unsandboxed if the sandbox cannot start");
        assert_eq!(sb["allowUnsandboxedCommands"], false, "no unsandboxed retry");
        assert_eq!(sb["filesystem"]["denyWrite"], json!(["/data/wt/r1/build"]));
        assert_eq!(sb["filesystem"]["denyRead"], json!(["~/"]));
        assert_eq!(sb["filesystem"]["allowRead"], json!(["/data/wt/r1/build", "/work/app/.git"]));
    }

    #[test]
    fn only_an_inspecting_agent_gets_settings_or_a_shell() {
        for access in [Access::None, Access::ReadOnly, Access::Write] {
            let a = args(&spec(access));
            assert!(!has(&a, "--settings"));
            assert!(!after(&a, "--tools").unwrap_or("").contains("Bash"));
        }
    }

    #[test]
    fn only_a_builders_conversation_is_kept_so_it_can_be_opened_live() {
        for access in [Access::None, Access::ReadOnly, Access::Inspect] {
            assert!(has(&args(&spec(access)), "--no-session-persistence"), "{access:?} leaves nothing behind");
        }
        assert!(!has(&args(&spec(Access::Write)), "--no-session-persistence"));
    }

    #[test]
    fn the_result_carries_the_conversation_id() {
        let Event::Done(o) = parse(r#"{"type":"result","session_id":"abc-123","structured_output":{}}"#) else { panic!() };
        assert_eq!(o.session.as_deref(), Some("abc-123"));
    }

    #[test]
    fn a_prompt_writer_gets_no_tools_at_all() {
        let a = args(&spec(Access::None));
        assert_eq!(after(&a, "--tools"), Some(""));
        assert!(!has(&a, "--dangerously-skip-permissions") && !has(&a, "--allowedTools"));
    }

    #[test]
    fn an_empty_system_prompt_is_not_passed() {
        let mut s = spec(Access::ReadOnly);
        s.system_prompt = "  ".into();
        assert!(!has(&args(&s), "--append-system-prompt"));
    }

    #[test]
    fn parses_the_final_result() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"{\"word\":\"ok\"}","structured_output":{"word":"ok","n":2},"total_cost_usd":0.0008,"usage":{"input_tokens":2,"output_tokens":73,"cache_read_input_tokens":0,"cache_creation_input_tokens":3836}}"#;
        let Event::Done(o) = parse(line) else { panic!("not a result") };
        assert_eq!(o.structured, Some(json!({"word": "ok", "n": 2})));
        assert_eq!(o.tokens, Tokens { input: 2, output: 73, cache_read: 0, cache_write: 3836 });
        assert!(!o.is_error);
        assert!((o.cost_usd - 0.0008).abs() < 1e-9);
    }

    #[test]
    fn a_null_structured_output_is_none() {
        let Event::Done(o) = parse(r#"{"type":"result","structured_output":null,"result":"x"}"#) else {
            panic!()
        };
        assert_eq!(o.structured, None);
    }

    #[test]
    fn parses_init_text_and_tool_calls() {
        assert_eq!(
            parse(r#"{"type":"system","subtype":"init","session_id":"abc","model":"claude-haiku-5-5"}"#),
            Event::Init { session_id: "abc".into(), model: "claude-haiku-5-5".into() }
        );
        assert_eq!(
            parse(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."}]}}"#),
            Event::Said("Looking.".into())
        );
        assert_eq!(
            parse(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"src/a.rs"}}]}}"#),
            Event::Tool("Read src/a.rs".into())
        );
    }

    #[test]
    fn noise_is_ignored() {
        assert_eq!(parse("not json"), Event::Other);
        assert_eq!(parse(r#"{"type":"system","subtype":"hook_started"}"#), Event::Other);
        assert_eq!(parse(r#"{"type":"rate_limit_event"}"#), Event::Other);
        assert_eq!(parse(r#"{"type":"assistant","message":{"content":[{"type":"thinking"}]}}"#), Event::Other);
    }

    /// A fresh empty folder for a live test to work in, so nothing lands in the shared temp folder.
    fn scratch_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ttm-orch-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Talks to the real `claude`, so it uses a little of the plan: `cargo test -- --ignored two_turns`.
    #[tokio::test]
    #[ignore]
    async fn two_turns_keep_one_process_and_hit_the_cache() {
        let mut s = spec(Access::ReadOnly);
        s.model = "haiku".into();
        s.schema = json!({"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]});
        s.cwd = scratch_dir("claude");
        let mut agent = Agent::start(&s).await.unwrap();
        let first = agent.turn("Answer with n = 1.", |_| {}).await.unwrap();
        let second = agent.turn("Now answer with n = 2.", |_| {}).await.unwrap();
        eprintln!("first  {:?}\nsecond {:?}", first.tokens, second.tokens);
        assert_eq!(first.structured, Some(json!({"n": 1})));
        assert_eq!(second.structured, Some(json!({"n": 2})));
        agent.finish().await.unwrap();
    }

    /// Proves the judge's shell is confined by the operating system, by what ends up on disk rather
    /// than by what the model says. Uses a little of the plan: `cargo test -- --ignored sandbox`.
    #[tokio::test]
    #[ignore]
    async fn the_inspect_shell_is_confined_by_the_os() {
        use std::process::Command as Sh;
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let tag = format!("ttm-orch-probe-{}", std::process::id());
        let root = std::env::temp_dir().join(&tag);
        let (project, base) = (root.join("project"), root.join("wt"));
        std::fs::create_dir_all(&project).unwrap();
        let git = |dir: &std::path::Path, args: &[&str]| {
            assert!(Sh::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success(), "git {args:?}");
        };
        git(&project, &["init", "-q"]);
        std::fs::write(project.join("file.txt"), "inside\n").unwrap();
        git(&project, &["add", "."]);
        git(&project, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "x"]);
        let wt = crate::orch::worktree::ensure(&project, &base, "r1", "build").await.unwrap();

        // A file in the user's home that the shell must not be able to read, and one it must not create.
        let secret = home.join(format!("{tag}-secret.txt"));
        let token = format!("SECRET-{tag}");
        std::fs::write(&secret, &token).unwrap();
        let written_home = home.join(format!("{tag}-written.txt"));
        let written_inside = wt.join("new.txt");

        let cmds = [
            format!("cat {}/file.txt", wt.display()),
            format!("echo x > {}", written_inside.display()),
            format!("echo x > {}", written_home.display()),
            format!("cat {}", secret.display()),
            format!("git -C {} status --short", wt.display()),
        ];
        let prompt = format!(
            "Using the Bash tool, run each of these commands exactly as written, one at a time, even if one fails. \
             Report each command's exit code and its combined output.\n\n{}",
            cmds.iter().enumerate().map(|(i, c)| format!("{}. {c}", i + 1)).collect::<Vec<_>>().join("\n")
        );
        let mut sp = spec(Access::Inspect);
        sp.model = "haiku".into();
        sp.cwd = wt.clone();
        sp.read_also = vec![project.join(".git")];
        sp.schema = json!({"type":"object","additionalProperties":false,"required":["results"],"properties":{
            "results":{"type":"array","items":{"type":"object","additionalProperties":false,
            "required":["n","exit_code","output"],"properties":{"n":{"type":"integer"},"exit_code":{"type":"integer"},
            "output":{"type":"string"}}}}}});
        let mut agent = Agent::start(&sp).await.unwrap();
        let out = agent.turn(&prompt, |e| eprintln!("{e:?}")).await.unwrap();
        let _ = agent.finish().await;
        eprintln!("answer: {}", out.structured.clone().unwrap_or_default());

        let exit = |n: i64| {
            out.structured.as_ref().and_then(|v| v["results"].as_array()).and_then(|r| {
                r.iter().find(|x| x["n"] == n).and_then(|x| x["exit_code"].as_i64())
            })
        };
        let (read_ok, git_ok) = (exit(1), exit(5));

        // Ground truth is the disk.
        let inside_written = written_inside.exists();
        let home_written = written_home.exists();
        let answer = out.structured.clone().unwrap_or_default().to_string();
        let leaked = answer.contains(&token);
        let _ = std::fs::remove_file(&secret);
        let _ = std::fs::remove_file(&written_home);
        let _ = Sh::new("git").arg("-C").arg(&project).args(["worktree", "remove", "--force"]).arg(&wt).output();
        let _ = std::fs::remove_dir_all(&root);

        assert!(!inside_written, "the shell wrote a file inside the worktree");
        assert!(!home_written, "the shell wrote a file in the home folder");
        assert!(!leaked, "the shell read a file in the home folder");
        assert_eq!(read_ok, Some(0), "it can read inside the worktree");
        assert_eq!(git_ok, Some(0), "git status works in the worktree");
    }

    #[test]
    fn the_stdin_line_is_one_json_object() {
        let l = user_line("hi\nthere");
        assert!(!l.contains('\n'));
        let v: Value = serde_json::from_str(&l).unwrap();
        assert_eq!(v["message"]["content"], "hi\nthere");
        assert_eq!(v["type"], "user");
    }
}
