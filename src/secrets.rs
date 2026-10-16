//! Provider accounts (OpenRouter, OpenAI-compatible endpoints) and their API keys. One JSON file in
//! the data dir, readable by the owner only. The page can set and clear a key but never read it
//! back: it gets `last4` instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

const FILE: &str = "accounts.json";
pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
const MAX_ACCOUNTS: usize = 16;
const MAX_MODELS: usize = 200;
const MAX_NAME: usize = 40;
const MAX_KEY: usize = 512;
const MAX_URL: usize = 300;
const MAX_MODEL_ID: usize = 200;

/// Serialises writes so two saves cannot interleave their temp files.
static WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Openrouter,
    OpenaiCompatible,
    /// Web search and scraping for agents. Has no models.
    Firecrawl,
    /// Web search, page reading, site maps and crawls. Has no models.
    Tavily,
    /// Google search and news. Has no models.
    Serper,
}

impl Kind {
    /// The web service an account of this kind is, if it is one.
    pub fn web(self) -> Option<crate::web::types::Provider> {
        use crate::web::types::Provider;
        match self {
            Kind::Firecrawl => Some(Provider::Firecrawl),
            Kind::Tavily => Some(Provider::Tavily),
            Kind::Serper => Some(Provider::Serper),
            Kind::Openrouter | Kind::OpenaiCompatible => None,
        }
    }

    pub fn is_web(self) -> bool {
        self.web().is_some()
    }
}

/// A model pinned to the shortlist. Prices are dollars per million tokens and optional: without
/// them an account shows tokens only and dollar caps do not apply to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PinnedModel {
    pub id: String,
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
    pub tools: bool,
}

impl Default for PinnedModel {
    fn default() -> Self {
        Self { id: String::new(), price_in: None, price_out: None, tools: true }
    }
}

/// How OpenRouter should route requests made with an account. Sent as the request's `provider` object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// `price`, `throughput` or `latency`; none lets OpenRouter balance them.
    pub sort: Option<String>,
    /// Only providers that do not keep or train on prompts.
    pub deny_data_collection: bool,
    /// Only zero-data-retention providers.
    pub zdr: bool,
    /// Let OpenRouter try another provider when the first fails.
    pub allow_fallbacks: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { sort: None, deny_data_collection: false, zdr: false, allow_fallbacks: true }
    }
}

impl Prefs {
    fn check(&self) -> Result<(), &'static str> {
        match self.sort.as_deref() {
            None | Some("price" | "throughput" | "latency") => Ok(()),
            Some(_) => Err("sort must be price, throughput or latency"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub base_url: String,
    pub key: String,
    /// Hides this account's models from every orchestrator picker, pool and helper call when off.
    pub use_in_orchestrator: bool,
    pub models: Vec<PinnedModel>,
    #[serde(default)]
    pub prefs: Prefs,
    /// What one of a web account's credits costs, in dollars, when the list price is not what is paid.
    #[serde(default)]
    pub usd_per_credit: Option<f64>,
}

/// What the page may see of an account: everything but the key.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub base_url: String,
    pub has_key: bool,
    pub last4: String,
    pub use_in_orchestrator: bool,
    pub models: Vec<PinnedModel>,
    pub prefs: Prefs,
    /// What a credit is taken to cost: the account's own figure or its service's list price. `None` for a model account.
    pub usd_per_credit: Option<f64>,
    /// Whether `usd_per_credit` is the account's own rather than the list price.
    pub usd_per_credit_set: bool,
}

/// A create or update from the page. A missing `key` keeps the stored one.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInput {
    pub name: String,
    pub kind: Kind,
    #[serde(default)]
    pub base_url: String,
    pub key: Option<String>,
    #[serde(default)]
    pub use_in_orchestrator: bool,
    #[serde(default)]
    pub models: Vec<PinnedModel>,
    #[serde(default)]
    pub prefs: Prefs,
    /// Dollars per credit for a web account; left out, the list price is used.
    #[serde(default)]
    pub usd_per_credit: Option<f64>,
}

/// A model on one of the accounts, chosen for a job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRef {
    pub account: String,
    pub model: String,
}

/// The calls ttm makes for itself, each of which may be given a model of its own. Left out, it uses Claude.
pub const HELPERS: [&str; 6] = ["suggest", "edit", "grill", "nodePrompt", "customRole", "route"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Accounts {
    pub accounts: Vec<Account>,
    /// Which model makes each helper call. See `HELPERS`.
    pub helpers: BTreeMap<String, ModelRef>,
    /// Which CLI runs the OpenRouter models of a vendor (`anthropic` → claude), overriding the built-in rule.
    pub drivers: BTreeMap<String, crate::openrouter::launch::Driver>,
    /// Pages (a leaderboard, a price comparison) read now and then to tell the router how models compare.
    pub intel_urls: Vec<String>,
    /// Which web accounts to ask first for each tool (`search`, `scrape`, `map`, `crawl`), by account id. A tool left
    /// out, or an account not listed, falls back to the built-in order.
    pub web_order: BTreeMap<String, Vec<String>>,
}

impl Account {
    pub fn view(&self) -> AccountView {
        let tail: Vec<char> = self.key.chars().rev().take(4).collect();
        AccountView {
            id: self.id.clone(),
            name: self.name.clone(),
            kind: self.kind,
            base_url: self.base_url.clone(),
            has_key: !self.key.is_empty(),
            last4: tail.into_iter().rev().collect(),
            use_in_orchestrator: self.use_in_orchestrator,
            models: self.models.clone(),
            prefs: self.prefs.clone(),
            usd_per_credit: self.kind.web().map(|_| self.credit_usd()),
            usd_per_credit_set: self.usd_per_credit.is_some(),
        }
    }

    /// What one of this account's credits costs, in dollars.
    pub fn credit_usd(&self) -> f64 {
        self.usd_per_credit.or(self.kind.web().map(|p| p.default_usd_per_credit())).unwrap_or(0.0)
    }
}

impl Accounts {
    pub fn views(&self) -> Vec<AccountView> {
        self.accounts.iter().map(Account::view).collect()
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// Add an account (`id` None) or change one. Returns the stored account's id.
    pub fn upsert(&mut self, id: Option<&str>, input: AccountInput) -> Result<String, &'static str> {
        let name = input.name.trim().to_owned();
        if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
            return Err("name must be 1 to 40 characters");
        }
        if self.accounts.iter().any(|a| a.name == name && Some(a.id.as_str()) != id) {
            return Err("an account with that name exists");
        }
        let base_url = match input.kind {
            Kind::Openrouter => OPENROUTER_URL.to_owned(),
            Kind::OpenaiCompatible => normalise_url(&input.base_url)?,
            // Blank means the hosted service; a self-hosted one is named by its address.
            Kind::Firecrawl | Kind::Tavily | Kind::Serper if input.base_url.trim().is_empty() => match input.kind {
                Kind::Tavily => crate::web::tavily::DEFAULT_URL,
                Kind::Serper => crate::web::serper::DEFAULT_URL,
                _ => crate::web::client::DEFAULT_URL,
            }
            .to_owned(),
            Kind::Firecrawl | Kind::Tavily | Kind::Serper => normalise_url(&input.base_url)?,
        };
        let models = if input.kind.is_web() { Vec::new() } else { check_models(input.models)? };
        let usd_per_credit = match input.usd_per_credit {
            Some(_) if !input.kind.is_web() => None,
            Some(x) if x.is_finite() && x > 0.0 && x <= 1.0 => Some(x),
            Some(_) => return Err("cost per credit must be more than zero, and at most one dollar"),
            None => None,
        };
        input.prefs.check()?;
        let old = match id {
            Some(id) => Some(self.get(id).ok_or("no such account")?.clone()),
            None => {
                if self.accounts.len() >= MAX_ACCOUNTS {
                    return Err("too many accounts");
                }
                None
            }
        };
        if let Some(o) = &old
            && o.kind != input.kind
        {
            return Err("an account's type cannot change");
        }
        let key = match input.key {
            Some(k) => check_key(&k)?,
            None => old.as_ref().map(|o| o.key.clone()).unwrap_or_default(),
        };
        let account = Account {
            id: old.as_ref().map(|o| o.id.clone()).unwrap_or_else(new_id),
            name,
            kind: input.kind,
            base_url,
            key,
            use_in_orchestrator: input.use_in_orchestrator,
            models,
            prefs: input.prefs,
            usd_per_credit,
        };
        let id = account.id.clone();
        match self.accounts.iter_mut().find(|a| a.id == id) {
            Some(slot) => *slot = account,
            None => self.accounts.push(account),
        }
        Ok(id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.id != id);
        // A helper that used the account has nothing to call any more.
        self.helpers.retain(|_, r| r.account != id);
        for list in self.web_order.values_mut() {
            list.retain(|a| a != id);
        }
        self.web_order.retain(|_, list| !list.is_empty());
        self.accounts.len() != before
    }

    /// The account a model may be called on from ttm's own jobs and from flows: it has a key, and is allowed in flows.
    pub fn usable(&self, account: &str) -> Option<&Account> {
        self.get(account).filter(|a| !a.key.is_empty() && a.use_in_orchestrator)
    }

    /// Replace the helper models and the CLI table. Each helper must name a usable account.
    pub fn set_settings(&mut self, helpers: BTreeMap<String, ModelRef>, drivers: BTreeMap<String, crate::openrouter::launch::Driver>) -> Result<(), &'static str> {
        for (name, r) in &helpers {
            if !HELPERS.contains(&name.as_str()) {
                return Err("unknown helper");
            }
            if self.usable(&r.account).is_none_or(|a| a.kind.is_web()) {
                return Err("a helper's account has no key, is not allowed in flows, or has no models");
            }
            if !crate::openrouter::launch::valid_model(&r.model) {
                return Err("a helper's model id is not valid");
            }
        }
        if drivers.len() > 64 || drivers.keys().any(|k| k.is_empty() || k.len() > 40 || !k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))) {
            return Err("a vendor name is not valid");
        }
        self.helpers = helpers;
        self.drivers = drivers;
        Ok(())
    }

    /// Choose the pages the router reads about how models compare. Only addresses a stranger could reach, a few of them.
    pub fn set_intel(&mut self, urls: Vec<String>) -> Result<(), &'static str> {
        let urls: Vec<String> = urls.into_iter().map(|u| u.trim().to_owned()).filter(|u| !u.is_empty()).collect();
        if urls.len() > crate::web::tools::MAX_LINKS {
            return Err("at most three pages");
        }
        if urls.iter().any(|u| crate::web::tools::url_ok(u).is_err()) {
            return Err("an address is not one that can be fetched");
        }
        self.intel_urls = urls;
        Ok(())
    }

    /// Choose which web accounts are asked first for each tool. Each must be a web account that can do the tool.
    pub fn set_web_order(&mut self, order: BTreeMap<String, Vec<String>>) -> Result<(), &'static str> {
        for (key, ids) in &order {
            let tool = crate::web::types::Tool::from_key(key).ok_or("unknown web tool")?;
            for (i, id) in ids.iter().enumerate() {
                let can = self.get(id).and_then(|a| a.kind.web()).is_some_and(|p| p.can(tool));
                if !can {
                    return Err("an account in the order is not a web account that can do that");
                }
                if ids[..i].contains(id) {
                    return Err("an account is listed twice");
                }
            }
        }
        self.web_order = order.into_iter().filter(|(_, ids)| !ids.is_empty()).collect();
        Ok(())
    }

    /// The web accounts agents may use: they have a key and are allowed in flows.
    pub fn web_accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.iter().filter(|a| a.kind.is_web() && !a.key.is_empty() && a.use_in_orchestrator)
    }

    /// The Firecrawl account agents use: the first with a key that is allowed in flows.
    pub fn firecrawl(&self) -> Option<&Account> {
        self.accounts.iter().find(|a| a.kind == Kind::Firecrawl && !a.key.is_empty() && a.use_in_orchestrator)
    }

    /// Where to send a helper's call, when the user chose a model for it and its account can still be used.
    pub fn helper_endpoint(&self, helper: &str) -> Option<crate::providers::chat::Endpoint> {
        let r = self.helpers.get(helper)?;
        Some(crate::providers::chat::endpoint_for(self.usable(&r.account)?, &r.model))
    }
}

fn new_id() -> String {
    let mut b = [0u8; 5];
    rand::fill(&mut b[..]);
    format!("a{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

/// An http(s) base URL with no trailing slash, query or fragment.
fn normalise_url(raw: &str) -> Result<String, &'static str> {
    let u = raw.trim().trim_end_matches('/');
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).ok_or("base URL must start with http:// or https://")?;
    if rest.is_empty() || u.len() > MAX_URL || u.chars().any(|c| c.is_control() || c.is_whitespace()) || u.contains('?') || u.contains('#') {
        return Err("base URL is not valid");
    }
    Ok(u.to_owned())
}

fn check_key(raw: &str) -> Result<String, &'static str> {
    let k = raw.trim();
    if k.len() > MAX_KEY || k.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("key is not valid");
    }
    Ok(k.to_owned())
}

fn check_models(models: Vec<PinnedModel>) -> Result<Vec<PinnedModel>, &'static str> {
    if models.len() > MAX_MODELS {
        return Err("too many pinned models");
    }
    let mut out: Vec<PinnedModel> = Vec::with_capacity(models.len());
    for mut m in models {
        m.id = m.id.trim().to_owned();
        if m.id.is_empty() || m.id.len() > MAX_MODEL_ID || m.id.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("model id is not valid");
        }
        if out.iter().any(|o| o.id == m.id) {
            return Err("a model is pinned twice");
        }
        for p in [m.price_in, m.price_out].into_iter().flatten() {
            if !p.is_finite() || !(0.0..1.0e6).contains(&p) {
                return Err("price is not valid");
            }
        }
        out.push(m);
    }
    Ok(out)
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// The stored accounts, or none when there is no file (or it cannot be read).
pub async fn load(data_dir: &Path) -> Accounts {
    let Ok(bytes) = tokio::fs::read(path(data_dir)).await else {
        return Accounts::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        tracing::warn!("{FILE} is unreadable; ignoring it");
        Accounts::default()
    })
}

/// Replace the file whole. Tests use it; the server goes through `update`.
#[cfg(test)]
pub async fn save(data_dir: &Path, a: &Accounts) -> Result<()> {
    let _guard = WRITE.lock().await;
    write(data_dir, a).await
}

/// Load, change and save under one lock, so two edits never overwrite each other. The closure's
/// error (a rejected edit) leaves the file alone.
pub async fn update<T>(
    data_dir: &Path,
    edit: impl FnOnce(&mut Accounts) -> Result<T, &'static str>,
) -> Result<Result<T, &'static str>> {
    let _guard = WRITE.lock().await;
    let mut a = load(data_dir).await;
    match edit(&mut a) {
        Ok(v) => {
            write(data_dir, &a).await?;
            Ok(Ok(v))
        }
        Err(why) => Ok(Err(why)),
    }
}

async fn write(data_dir: &Path, a: &Accounts) -> Result<()> {
    let path = path(data_dir);
    let bytes = serde_json::to_vec_pretty(a)?;
    tokio::task::spawn_blocking(move || write_private(&path, &bytes)).await??;
    Ok(())
}

/// Write a file only its owner can read, through a temp file so a crash never leaves half of it.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    // Its own temporary file each time: two writers sharing one name make the second rename fail, or mix their bytes.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!("json.{}.{}.tmp", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_writers_at_once_all_succeed_and_leave_one_whole_file_and_no_litter() {
        let dir = std::env::temp_dir().join(format!("ttm-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.json");
        let threads: Vec<_> = (0..16)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || (0..40).map(|j| write_private(&path, format!("{{\"w\":{i},\"n\":{j},\"pad\":\"{}\"}}", "x".repeat(4000)).as_bytes())).collect::<Vec<_>>())
            })
            .collect();
        for t in threads {
            assert!(t.join().unwrap().iter().all(|r| r.is_ok()), "a writer lost the race");
        }
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).expect("the file is one whole write");
        assert!(v["w"].is_number());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary files are left behind");
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions()) & 0o777, 0o600);
        std::fs::remove_dir_all(dir).ok();
    }

    fn input(name: &str, kind: Kind) -> AccountInput {
        AccountInput {
            name: name.into(),
            kind,
            base_url: "http://localhost:8000/v1/".into(),
            key: Some("sk-or-abcdef1234".into()),
            use_in_orchestrator: true,
            models: vec![PinnedModel { id: "anthropic/claude-sonnet-5".into(), ..Default::default() }],
            prefs: Prefs::default(),
            usd_per_credit: None,
        }
    }

    #[test]
    fn view_never_carries_the_key() {
        let mut a = Accounts::default();
        let id = a.upsert(None, input("OR", Kind::Openrouter)).unwrap();
        let v = a.get(&id).unwrap().view();
        assert!(v.has_key);
        assert_eq!(v.last4, "1234");
        assert!(!serde_json::to_string(&v).unwrap().contains("abcdef"));
    }

    #[test]
    fn openrouter_url_is_fixed_and_others_are_normalised() {
        let mut a = Accounts::default();
        let or = a.upsert(None, input("OR", Kind::Openrouter)).unwrap();
        assert_eq!(a.get(&or).unwrap().base_url, OPENROUTER_URL);
        let oa = a.upsert(None, input("Local", Kind::OpenaiCompatible)).unwrap();
        assert_eq!(a.get(&oa).unwrap().base_url, "http://localhost:8000/v1");
        let mut bad = input("Bad", Kind::OpenaiCompatible);
        bad.base_url = "ftp://x".into();
        assert!(a.upsert(None, bad).is_err());
        let mut bad = input("Bad", Kind::OpenaiCompatible);
        bad.base_url = "https://x/v1?k=1".into();
        assert!(a.upsert(None, bad).is_err());
    }

    #[test]
    fn update_keeps_the_key_unless_replaced() {
        let mut a = Accounts::default();
        let id = a.upsert(None, input("OR", Kind::Openrouter)).unwrap();
        let mut i = input("OR renamed", Kind::Openrouter);
        i.key = None;
        a.upsert(Some(&id), i).unwrap();
        assert_eq!(a.get(&id).unwrap().key, "sk-or-abcdef1234");
        assert_eq!(a.get(&id).unwrap().name, "OR renamed");
        let mut i = input("OR renamed", Kind::Openrouter);
        i.key = Some(String::new());
        a.upsert(Some(&id), i).unwrap();
        assert!(!a.get(&id).unwrap().view().has_key, "an empty key clears it");
    }

    #[test]
    fn rejects_bad_input() {
        let mut a = Accounts::default();
        let id = a.upsert(None, input("A", Kind::Openrouter)).unwrap();
        assert!(a.upsert(None, input("A", Kind::Openrouter)).is_err(), "names are unique");
        assert!(a.upsert(None, input("  ", Kind::Openrouter)).is_err());
        assert!(a.upsert(Some(&id), input("A", Kind::OpenaiCompatible)).is_err(), "kind cannot change");
        assert!(a.upsert(Some("nope"), input("B", Kind::Openrouter)).is_err());
        let mut i = input("B", Kind::Openrouter);
        i.key = Some("has space".into());
        assert!(a.upsert(None, i).is_err());
        let mut i = input("B", Kind::Openrouter);
        i.models.push(i.models[0].clone());
        assert!(a.upsert(None, i).is_err(), "no duplicate models");
        let mut i = input("B", Kind::Openrouter);
        i.models[0].price_in = Some(f64::NAN);
        assert!(a.upsert(None, i).is_err());
        let mut i = input("B", Kind::Openrouter);
        i.models[0].price_out = Some(-1.0);
        assert!(a.upsert(None, i).is_err());
    }

    #[test]
    fn routing_preferences_are_checked_and_old_files_still_load() {
        let mut a = Accounts::default();
        let mut i = input("OR", Kind::Openrouter);
        i.prefs = Prefs { sort: Some("fastest".into()), ..Prefs::default() };
        assert!(a.upsert(None, i).is_err());
        let mut i = input("OR", Kind::Openrouter);
        i.prefs = Prefs { sort: Some("price".into()), deny_data_collection: true, ..Prefs::default() };
        let id = a.upsert(None, i).unwrap();
        assert_eq!(a.get(&id).unwrap().view().prefs.sort.as_deref(), Some("price"));
        assert!(Prefs::default().allow_fallbacks, "fallbacks are on unless turned off");
        let old = r#"{"accounts":[{"id":"a1","name":"N","kind":"openrouter","baseUrl":"u","key":"k","useInOrchestrator":true,"models":[]}]}"#;
        let loaded: Accounts = serde_json::from_str(old).unwrap();
        assert_eq!(loaded.accounts[0].prefs, Prefs::default());
    }

    fn with_account(kind: Kind, flows: bool) -> (Accounts, String) {
        let mut a = Accounts::default();
        let mut i = input("A", kind);
        i.use_in_orchestrator = flows;
        i.models = vec![PinnedModel { id: "vendor/m".into(), price_in: Some(2.0), price_out: Some(8.0), tools: true }];
        let id = a.upsert(None, i).unwrap();
        (a, id)
    }

    #[test]
    fn helper_models_must_be_on_an_account_that_can_be_used() {
        let (mut a, id) = with_account(Kind::OpenaiCompatible, true);
        let r = |account: &str, model: &str| ModelRef { account: account.into(), model: model.into() };
        let one = |name: &str, r: ModelRef| BTreeMap::from([(name.to_string(), r)]);
        assert_eq!(a.set_settings(one("suggest", r(&id, "vendor/m")), BTreeMap::new()), Ok(()));
        assert!(a.set_settings(one("frobnicate", r(&id, "vendor/m")), BTreeMap::new()).is_err(), "only known helpers");
        assert!(a.set_settings(one("suggest", r("nope", "vendor/m")), BTreeMap::new()).is_err());
        assert!(a.set_settings(one("suggest", r(&id, "a b")), BTreeMap::new()).is_err());
        let bad_vendor = BTreeMap::from([("Bad Vendor".to_string(), crate::openrouter::launch::Driver::Claude)]);
        assert!(a.set_settings(BTreeMap::new(), bad_vendor).is_err());
        assert!(a.helpers.contains_key("suggest"), "a refused change leaves the settings as they were");

        let (mut off, off_id) = with_account(Kind::Openrouter, false);
        assert!(off.set_settings(one("grill", r(&off_id, "vendor/m")), BTreeMap::new()).is_err(), "not allowed in flows means not for helpers either");
    }

    #[test]
    fn a_helper_endpoint_carries_the_accounts_price_and_goes_away_with_the_account() {
        let (mut a, id) = with_account(Kind::OpenaiCompatible, true);
        a.accounts[0].key = "sk-1".into();
        a.accounts[0].base_url = "https://example.test/v1".into();
        let refs = BTreeMap::from([("edit".to_string(), ModelRef { account: id.clone(), model: "vendor/m".into() })]);
        a.set_settings(refs, BTreeMap::from([("google".to_string(), crate::openrouter::launch::Driver::Claude)])).unwrap();
        let e = a.helper_endpoint("edit").expect("an endpoint");
        assert_eq!((e.base_url.as_str(), e.model.as_str(), e.price_in, e.price_out, e.openrouter), ("https://example.test/v1", "vendor/m", Some(2.0), Some(8.0), false));
        assert!(a.helper_endpoint("grill").is_none(), "no model chosen for it");
        a.accounts[0].use_in_orchestrator = false;
        assert!(a.helper_endpoint("edit").is_none(), "turning the account off in flows turns its helpers off");
        a.accounts[0].use_in_orchestrator = true;
        assert!(a.remove(&id));
        assert!(a.helpers.is_empty(), "removing the account removes what pointed at it");
        assert_eq!(a.drivers.len(), 1, "the vendor table is not tied to an account");
    }

    #[test]
    fn settings_written_before_these_existed_still_load() {
        let old = r#"{"accounts":[]}"#;
        let a: Accounts = serde_json::from_str(old).unwrap();
        assert!(a.helpers.is_empty() && a.drivers.is_empty());
    }

    #[test]
    fn a_firecrawl_account_has_a_default_address_no_models_and_is_found_for_agents() {
        let mut a = Accounts::default();
        let mut i = input("Web", Kind::Firecrawl);
        i.base_url = String::new();
        i.models = vec![PinnedModel { id: "x/y".into(), ..Default::default() }];
        let id = a.upsert(None, i).unwrap();
        let acct = a.get(&id).unwrap();
        assert_eq!(acct.base_url, crate::web::client::DEFAULT_URL);
        assert!(acct.models.is_empty(), "it has no models to pin");
        assert!(a.firecrawl().is_some());
        a.accounts[0].use_in_orchestrator = false;
        assert!(a.firecrawl().is_none(), "off in flows means agents do not get it");
        a.accounts[0].use_in_orchestrator = true;
        a.accounts[0].key.clear();
        assert!(a.firecrawl().is_none(), "no key, no web");
        let mut own = input("Self", Kind::Firecrawl);
        own.base_url = "http://localhost:3002/".into();
        let id2 = a.upsert(None, own).unwrap();
        assert_eq!(a.get(&id2).unwrap().base_url, "http://localhost:3002");
    }

    fn web(a: &mut Accounts, name: &str, kind: Kind) -> String {
        let mut i = input(name, kind);
        i.base_url = String::new();
        a.upsert(None, i).unwrap()
    }

    #[test]
    fn tavily_and_serper_accounts_get_their_own_addresses_and_list_prices() {
        let mut a = Accounts::default();
        let (fc, tv, sv) = (web(&mut a, "Fc", Kind::Firecrawl), web(&mut a, "Tv", Kind::Tavily), web(&mut a, "Sv", Kind::Serper));
        assert_eq!(a.get(&tv).unwrap().base_url, crate::web::tavily::DEFAULT_URL);
        assert_eq!(a.get(&sv).unwrap().base_url, crate::web::serper::DEFAULT_URL);
        assert!(a.get(&tv).unwrap().models.is_empty() && a.get(&sv).unwrap().models.is_empty());
        let rates: Vec<f64> = [&fc, &tv, &sv].iter().map(|id| a.get(id).unwrap().credit_usd()).collect();
        assert_eq!(rates, [0.00083, 0.008, 0.001]);
        assert_eq!(a.web_accounts().count(), 3);
        let v = a.get(&tv).unwrap().view();
        assert_eq!((v.usd_per_credit, v.usd_per_credit_set), (Some(0.008), false), "the page is shown the price in use, and that it is the list price");
        let mut own = input("Tv2", Kind::Tavily);
        own.base_url = "http://localhost:9/".into();
        let id = a.upsert(None, own).unwrap();
        assert_eq!(a.get(&id).unwrap().base_url, "http://localhost:9", "a named address is kept, without its trailing slash");
    }

    #[test]
    fn a_web_accounts_own_price_per_credit_is_kept_and_checked() {
        let mut a = Accounts::default();
        let mut i = input("Tv", Kind::Tavily);
        i.base_url = String::new();
        i.usd_per_credit = Some(0.005);
        let id = a.upsert(None, i.clone()).unwrap();
        assert_eq!(a.get(&id).unwrap().credit_usd(), 0.005);
        let v = a.get(&id).unwrap().view();
        assert_eq!((v.usd_per_credit, v.usd_per_credit_set), (Some(0.005), true));
        for bad in [0.0, -1.0, 1.5, f64::NAN, f64::INFINITY] {
            i.usd_per_credit = Some(bad);
            assert!(a.upsert(Some(&id), i.clone()).is_err(), "{bad}");
        }
        i.usd_per_credit = None;
        a.upsert(Some(&id), i).unwrap();
        assert_eq!(a.get(&id).unwrap().credit_usd(), 0.008, "cleared, it is the list price again");
        let mut model = input("OR", Kind::Openrouter);
        model.usd_per_credit = Some(0.5);
        let m = a.upsert(None, model).unwrap();
        assert_eq!((a.get(&m).unwrap().usd_per_credit, a.get(&m).unwrap().view().usd_per_credit), (None, None), "a model account has no credits");
    }

    #[test]
    fn web_accounts_are_not_offered_as_model_accounts() {
        let mut a = Accounts::default();
        let tv = web(&mut a, "Tv", Kind::Tavily);
        let helpers = BTreeMap::from([("suggest".to_string(), ModelRef { account: tv, model: "x/y".into() })]);
        assert!(a.set_settings(helpers, BTreeMap::new()).is_err());
    }

    #[test]
    fn the_order_of_web_accounts_is_per_tool_and_only_names_accounts_that_can_do_it() {
        let mut a = Accounts::default();
        let (fc, tv, sv) = (web(&mut a, "Fc", Kind::Firecrawl), web(&mut a, "Tv", Kind::Tavily), web(&mut a, "Sv", Kind::Serper));
        let order = |pairs: &[(&str, &[&String])]| pairs.iter().map(|(k, ids)| (k.to_string(), ids.iter().map(|i| (*i).clone()).collect())).collect::<BTreeMap<String, Vec<String>>>();
        assert_eq!(a.set_web_order(order(&[("search", &[&fc, &sv]), ("scrape", &[&tv])])), Ok(()));
        assert_eq!(a.web_order["search"], [fc.clone(), sv.clone()]);
        assert!(a.set_web_order(order(&[("scrape", &[&sv])])).is_err(), "Serper cannot scrape");
        assert!(a.set_web_order(order(&[("search", &[&fc, &fc])])).is_err(), "listed twice");
        assert!(a.set_web_order(order(&[("search", &[&"nope".to_string()])])).is_err());
        assert!(a.set_web_order(order(&[("browse", &[&fc])])).is_err(), "no such tool");
        assert_eq!(a.web_order["search"], [fc.clone(), sv.clone()], "a refused change leaves it as it was");
        assert_eq!(a.set_web_order(order(&[("map", &[])])), Ok(()));
        assert!(a.web_order.is_empty(), "an empty list is the built-in order");
        a.set_web_order(order(&[("search", &[&fc, &sv]), ("crawl", &[&tv, &fc])])).unwrap();
        assert!(a.remove(&sv));
        assert_eq!(a.web_order["search"], [fc.clone()], "a removed account leaves the order");
        assert!(a.remove(&fc) && a.remove(&tv));
        assert!(a.web_order.is_empty());
    }

    #[test]
    fn accounts_saved_before_the_price_and_the_order_still_load() {
        let old = r#"{"accounts":[{"id":"a1","name":"Web","kind":"firecrawl","baseUrl":"https://api.firecrawl.dev","key":"k","useInOrchestrator":true,"models":[]}]}"#;
        let a: Accounts = serde_json::from_str(old).unwrap();
        assert_eq!((a.accounts[0].usd_per_credit, a.web_order.len()), (None, 0));
        assert_eq!(a.accounts[0].credit_usd(), 0.00083);
    }

    #[test]
    fn the_pages_the_router_reads_are_few_and_public() {
        let mut a = Accounts::default();
        assert_eq!(a.set_intel(vec!["  https://a.test/board ".into(), "".into()]), Ok(()));
        assert_eq!(a.intel_urls, ["https://a.test/board"]);
        assert!(a.set_intel(vec!["http://localhost/x".into()]).is_err());
        assert!(a.set_intel(vec!["https://1.test".into(), "https://2.test".into(), "https://3.test".into(), "https://4.test".into()]).is_err());
        assert_eq!(a.intel_urls, ["https://a.test/board"], "a refused change leaves it as it was");
    }

    #[test]
    fn caps_the_number_of_accounts() {
        let mut a = Accounts::default();
        for n in 0..MAX_ACCOUNTS {
            a.upsert(None, input(&format!("a{n}"), Kind::Openrouter)).unwrap();
        }
        assert!(a.upsert(None, input("one more", Kind::Openrouter)).is_err());
    }

    #[test]
    fn remove_reports_whether_it_found_one() {
        let mut a = Accounts::default();
        let id = a.upsert(None, input("A", Kind::Openrouter)).unwrap();
        assert!(a.remove(&id));
        assert!(!a.remove(&id));
    }

    #[tokio::test]
    async fn round_trips_and_is_owner_only() {
        let dir = std::env::temp_dir().join(format!("ttm-accounts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir).await, Accounts::default());
        let mut a = Accounts::default();
        a.upsert(None, input("OR", Kind::Openrouter)).unwrap();
        save(&dir, &a).await.unwrap();
        assert_eq!(load(&dir).await, a);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(dir.join(FILE), b"not json").unwrap();
        assert_eq!(load(&dir).await, Accounts::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
