//! Starting an agent CLI on an OpenRouter model: which CLI runs it, and what that CLI is given.
//!
//! Claude Code is pointed at the proxy through a per-session `--settings` file, not through the
//! environment: an `env` block in the user's own `~/.claude/settings.json` (a Team Claude
//! `ANTHROPIC_BASE_URL`, say) beats the process environment, and would quietly send the session
//! somewhere else. A `--settings` file on the command line outranks it. Codex takes `-c` overrides,
//! which already outrank its config file, and reads the lease token from its environment.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::proxy;
use crate::secrets::{self, Accounts, Kind};

const DIR: &str = "or-settings";

/// The CLI that runs an OpenRouter model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Driver {
    Claude,
    Codex,
}

impl Driver {
    /// What the session's command starts from, before any model flags.
    pub fn command(self) -> &'static str {
        match self {
            Driver::Claude => "claude",
            Driver::Codex => "codex",
        }
    }
}

/// Claude Code speaks Anthropic's API, so it runs Anthropic's models; everything else goes to Codex,
/// which speaks the OpenAI Responses API that OpenRouter serves for every vendor. A session can
/// still be started on the other CLI by asking for it.
pub fn driver_for(model: &str) -> Driver {
    driver_for_with(model, &Default::default())
}

/// Like `driver_for`, with the user's own table of vendors first.
pub fn driver_for_with(model: &str, table: &std::collections::BTreeMap<String, Driver>) -> Driver {
    let vendor = model.split('/').next().unwrap_or("").trim_start_matches('~');
    table.get(vendor).copied().unwrap_or(if vendor == "anthropic" { Driver::Claude } else { Driver::Codex })
}

/// How an OpenRouter-backed session is started again after a restore. The model here is the one it
/// was last switched to; the lease holds the live one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrSpec {
    pub account: String,
    pub model: String,
    pub driver: Driver,
    #[serde(default)]
    pub cap_usd: Option<f64>,
}

/// A model id that is safe to put on a command line.
pub fn valid_model(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || "/._:-~+".contains(c))
}

/// Where agents on this machine reach the proxy: always loopback, whatever address ttm is served on.
pub fn origin(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Claude Code settings for one session: ttm's own hooks and status line, plus the proxy as the
/// API endpoint with the lease as its credential. The empty `ANTHROPIC_API_KEY` stops a key set
/// elsewhere from being preferred to the token.
pub fn claude_settings(hooks: &str, origin: &str, token: &str) -> Result<String> {
    let mut settings: Value = serde_json::from_str(hooks).context("ttm's hook settings")?;
    let env: serde_json::Map<String, Value> = proxy::claude_env(origin, token).into_iter().map(|(k, v)| (k.to_owned(), Value::String(v))).collect();
    settings["env"] = Value::Object(env);
    Ok(settings.to_string())
}

fn settings_path(data_dir: &Path, uid: &str) -> PathBuf {
    data_dir.join(DIR).join(format!("{uid}.json"))
}

/// Write the settings file for session `uid` (readable by you only: it holds the lease token).
pub fn write_claude_settings(data_dir: &Path, uid: &str, hooks: &str, origin: &str, token: &str) -> Result<PathBuf> {
    anyhow::ensure!(uid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !uid.is_empty(), "bad session uid");
    let path = settings_path(data_dir, uid);
    std::fs::create_dir_all(path.parent().context("settings dir")?)?;
    secrets::write_private(&path, claude_settings(hooks, origin, token)?.as_bytes())?;
    Ok(path)
}

/// Forget a session's settings file.
pub fn remove_claude_settings(data_dir: &Path, uid: &str) {
    let _ = std::fs::remove_file(settings_path(data_dir, uid));
}

/// Quote for `sh`.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The flags to add after the session's base command, and the environment to start it with.
pub struct Flags {
    pub args: String,
    pub env: Vec<(String, String)>,
}

/// Flags for Codex: the model, and the overrides that make the proxy its provider.
pub fn codex_flags(origin: &str, token: &str, model: &str) -> Flags {
    let mut args = format!(" -m {}", q(model));
    for a in proxy::codex_args(origin) {
        args.push(' ');
        // A bare flag such as `-c` needs no quotes; its value does.
        if a.starts_with('-') && a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            args.push_str(&a);
        } else {
            args.push_str(&q(&a));
        }
    }
    Flags { args, env: vec![(proxy::TOKEN_ENV.to_owned(), token.to_owned())] }
}

/// Flags for Claude Code: the settings file and the model.
pub fn claude_flags(settings: &Path, model: &str) -> Flags {
    Flags { args: format!(" --settings {} --model {}", q(&settings.display().to_string()), q(model)), env: Vec::new() }
}

/// A pinned OpenRouter model as a flow's agent pickers offer it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowModel {
    pub account: String,
    pub account_name: String,
    pub id: String,
    /// The CLI that runs it unless a node asks for the other.
    pub driver: Driver,
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
    pub tools: bool,
    /// Called directly, with no CLI and no tools. Every model of an OpenAI-compatible account is, since no CLI can run it.
    pub direct: bool,
}

/// The models flows may use: every pinned model of every account that has a key and is allowed in flows. Those on
/// OpenRouter run in a CLI unless asked otherwise; those on any other endpoint can only be called directly.
pub fn flow_models(accounts: &Accounts) -> Vec<FlowModel> {
    accounts
        .accounts
        .iter()
        .filter(|a| a.use_in_orchestrator && !a.key.is_empty())
        .flat_map(|a| {
            a.models.iter().map(|m| FlowModel {
                account: a.id.clone(),
                account_name: a.name.clone(),
                id: m.id.clone(),
                driver: driver_for_with(&m.id, &accounts.drivers),
                price_in: m.price_in,
                price_out: m.price_out,
                tools: m.tools,
                direct: a.kind == Kind::OpenaiCompatible,
            })
        })
        .collect()
}

/// One agent that runs on an account: its name, the account, the model, and whether it calls the model directly.
pub type OnAccount<'a> = (&'a str, &'a str, &'a str, bool);

/// Why a node cannot run on its account, or `Ok` when it can (or is not on one).
pub fn check_accounts<'a>(accounts: &Accounts, nodes: impl IntoIterator<Item = OnAccount<'a>>) -> Result<(), String> {
    for (name, account, model, direct) in nodes {
        let Some(a) = accounts.get(account) else {
            return Err(format!("{name} runs on an account that no longer exists"));
        };
        let why = if a.kind == Kind::Firecrawl {
            Some("is a web account, not one that has models")
        } else if a.kind == Kind::OpenaiCompatible && !direct {
            // No CLI can run a model on a plain OpenAI-compatible endpoint, so such an agent must call it itself.
            Some("can only be called directly, with no tools")
        } else if a.key.is_empty() {
            Some("has no key")
        } else if !a.use_in_orchestrator {
            Some("is not allowed in flows (see Settings, Account)")
        } else if !valid_model(model) {
            Some("was given a model id that cannot be used")
        } else {
            None
        };
        if let Some(why) = why {
            return Err(format!("{name} runs on the account “{}”, which {why}", a.name));
        }
    }
    Ok(())
}

/// What a session's page shows about its OpenRouter lease.
pub fn summary(spec: &OrSpec, lease: Option<&proxy::Lease>) -> Value {
    json!({
        "account": spec.account,
        "model": lease.map_or(spec.model.as_str(), |l| l.model.as_str()),
        "driver": spec.driver,
        "cap_usd": lease.map_or(spec.cap_usd, |l| l.cap_usd),
        "spent_usd": lease.map_or(0.0, |l| l.spent.cost_usd),
        "requests": lease.map_or(0, |l| l.spent.requests),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOOKS: &str = r#"{"statusLine":{"type":"command","command":"ttm statusline"},"hooks":{"Stop":[]}}"#;

    #[test]
    fn anthropic_models_go_to_claude_and_the_rest_to_codex() {
        assert_eq!(driver_for("anthropic/claude-haiku-5.5"), Driver::Claude);
        assert_eq!(driver_for("~anthropic/claude-haiku-latest"), Driver::Claude);
        assert_eq!(driver_for("openai/gpt-6-luna"), Driver::Codex);
        assert_eq!(driver_for("google/gemini-3-pro"), Driver::Codex);
        assert_eq!(driver_for("openrouter/auto"), Driver::Codex);
        assert_eq!(driver_for("no-vendor"), Driver::Codex);
    }

    #[test]
    fn the_users_table_of_vendors_comes_first() {
        let table = [("google".to_string(), Driver::Claude), ("anthropic".to_string(), Driver::Codex)].into_iter().collect();
        assert_eq!(driver_for_with("google/gemini-3", &table), Driver::Claude);
        assert_eq!(driver_for_with("~anthropic/claude-haiku-latest", &table), Driver::Codex, "the ~ alias follows its vendor");
        assert_eq!(driver_for_with("openai/gpt-6-luna", &table), Driver::Codex, "a vendor not in the table keeps the built-in rule");
    }

    #[test]
    fn only_command_line_safe_ids_pass() {
        for ok in ["anthropic/claude-haiku-5.5", "meta-llama/llama-3.3-70b-instruct:free", "~anthropic/claude-sonnet-latest", "a/b+c_d"] {
            assert!(valid_model(ok), "{ok}");
        }
        for bad in ["", "a b", "a;rm -rf", "a$(x)", "a'b", "a\"b", "a\nb", &"x".repeat(201)] {
            assert!(!valid_model(bad), "{bad:?}");
        }
    }

    #[test]
    fn claude_settings_keep_the_hooks_and_add_the_proxy() {
        let s: Value = serde_json::from_str(&claude_settings(HOOKS, "http://127.0.0.1:7681", "ttm-or-t").unwrap()).unwrap();
        assert_eq!(s["statusLine"]["command"], "ttm statusline", "hooks survive");
        assert!(s["hooks"].is_object());
        assert_eq!(s["env"]["ANTHROPIC_BASE_URL"], "http://127.0.0.1:7681/or/anthropic");
        assert_eq!(s["env"]["ANTHROPIC_AUTH_TOKEN"], "ttm-or-t");
        assert_eq!(s["env"]["ANTHROPIC_API_KEY"], "");
        assert!(claude_settings("not json", "o", "t").is_err());
    }

    #[test]
    fn the_settings_file_is_private_and_goes_away() {
        let dir = std::env::temp_dir().join(format!("ttm-orset-{}", std::process::id()));
        let path = write_claude_settings(&dir, "u1", HOOKS, "http://127.0.0.1:1", "ttm-or-secret").unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("ttm-or-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        remove_claude_settings(&dir, "u1");
        assert!(!path.exists());
        assert!(write_claude_settings(&dir, "../evil", HOOKS, "o", "t").is_err(), "uids cannot climb out of the folder");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn flags_quote_everything_and_keep_the_token_off_the_command_line() {
        let c = codex_flags("http://127.0.0.1:7681", "ttm-or-secret", "openai/gpt-6-luna");
        assert!(c.args.starts_with(" -m 'openai/gpt-6-luna' -c 'model_providers.ttm_or={"));
        assert!(c.args.contains("model_provider=\"ttm_or\""));
        assert!(!c.args.contains("ttm-or-secret"));
        assert_eq!(c.env, vec![("TTM_OR_TOKEN".to_owned(), "ttm-or-secret".to_owned())]);
        let k = claude_flags(Path::new("/data/or settings/u.json"), "anthropic/claude-haiku-5.5");
        assert_eq!(k.args, " --settings '/data/or settings/u.json' --model 'anthropic/claude-haiku-5.5'");
        assert!(k.env.is_empty(), "Claude's token travels in the settings file");
        assert_eq!(q("it's"), r"'it'\''s'");
    }

    fn account(id: &str, kind: Kind, key: &str, flows: bool, models: &[&str]) -> secrets::Account {
        secrets::Account {
            id: id.into(),
            name: format!("{id}-name"),
            kind,
            base_url: "u".into(),
            key: key.into(),
            use_in_orchestrator: flows,
            models: models.iter().map(|m| secrets::PinnedModel { id: (*m).into(), price_in: Some(1.0), price_out: None, tools: true }).collect(),
            prefs: secrets::Prefs::default(),
        }
    }

    #[test]
    fn flows_are_offered_only_the_models_they_may_use() {
        let accounts = Accounts {
            helpers: Default::default(),
            drivers: Default::default(),
            intel_urls: Default::default(),
            accounts: vec![
                account("ok", Kind::Openrouter, "k", true, &["anthropic/claude-haiku-5.5", "openai/gpt-6-luna"]),
                account("off", Kind::Openrouter, "k", false, &["x/hidden"]),
                account("nokey", Kind::Openrouter, "", true, &["x/nokey"]),
                account("oai", Kind::OpenaiCompatible, "k", true, &["x/direct"]),
            ],
        };
        let m = flow_models(&accounts);
        assert_eq!(m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["anthropic/claude-haiku-5.5", "openai/gpt-6-luna", "x/direct"]);
        assert_eq!((m[0].driver, m[1].driver, m[0].account_name.as_str()), (Driver::Claude, Driver::Codex, "ok-name"));
        assert_eq!(m.iter().map(|m| m.direct).collect::<Vec<_>>(), [false, false, true], "a plain endpoint's models can only be called directly");
        let json = serde_json::to_value(&m[0]).unwrap();
        assert_eq!((json["accountName"].as_str(), json["driver"].as_str(), json["priceIn"].as_f64()), (Some("ok-name"), Some("claude"), Some(1.0)));
    }

    #[test]
    fn a_node_whose_account_cannot_serve_it_is_refused_with_a_reason() {
        let accounts = Accounts {
            helpers: Default::default(),
            drivers: Default::default(),
            intel_urls: Default::default(),
            accounts: vec![
                account("ok", Kind::Openrouter, "k", true, &[]),
                account("off", Kind::Openrouter, "k", false, &[]),
                account("nokey", Kind::Openrouter, "", true, &[]),
                account("oai", Kind::OpenaiCompatible, "k", true, &[]),
            ],
        };
        assert_eq!(check_accounts(&accounts, [("Plan", "ok", "a/b", false)]), Ok(()));
        assert_eq!(check_accounts(&accounts, [("Plan", "ok", "a/b", true)]), Ok(()), "an OpenRouter model may also be called directly");
        assert_eq!(check_accounts(&accounts, std::iter::empty()), Ok(()));
        let why = |a: &'static str, m: &'static str| check_accounts(&accounts, [("Build", a, m, false)]).unwrap_err();
        assert!(why("gone", "a/b").contains("no longer exists"));
        assert!(why("off", "a/b").contains("not allowed in flows"));
        assert!(why("nokey", "a/b").contains("no key"));
        assert!(why("oai", "a/b").contains("only be called directly"), "no CLI runs a plain endpoint's models");
        assert_eq!(check_accounts(&accounts, [("Think", "oai", "a/b", true)]), Ok(()));
        let mut with_web = accounts.clone();
        with_web.accounts.push(account("web", Kind::Firecrawl, "k", true, &[]));
        assert!(check_accounts(&with_web, [("Think", "web", "a/b", true)]).unwrap_err().contains("web account"));
        assert!(why("ok", "a b").contains("cannot be used"));
        assert!(why("off", "a/b").contains("off-name"), "names the account");
    }

    #[test]
    fn summary_prefers_the_live_lease() {
        let spec = OrSpec { account: "a".into(), model: "x/old".into(), driver: Driver::Codex, cap_usd: Some(1.0) };
        let s = summary(&spec, None);
        assert_eq!((s["model"].as_str(), s["spent_usd"].as_f64(), s["driver"].as_str()), (Some("x/old"), Some(0.0), Some("codex")));
        let leases = proxy::Leases::new();
        let token = leases.mint("a", "x/new", "t", Some(2.0));
        leases.record(&token, &proxy::Usage { cost: Some(0.5), ..Default::default() });
        let s = summary(&spec, leases.get(&token).as_ref());
        assert_eq!((s["model"].as_str(), s["cap_usd"].as_f64(), s["spent_usd"].as_f64(), s["requests"].as_u64()), (Some("x/new"), Some(2.0), Some(0.5), Some(1)));
    }
}
