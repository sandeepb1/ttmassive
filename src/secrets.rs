//! Provider accounts (OpenRouter, OpenAI-compatible endpoints) and their API keys. One JSON file in
//! the data dir, readable by the owner only. The page can set and clear a key but never read it
//! back: it gets `last4` instead.

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
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Accounts {
    pub accounts: Vec<Account>,
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
        }
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
        };
        let models = check_models(input.models)?;
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
        self.accounts.len() != before
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
    let tmp = path.with_extension("json.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, kind: Kind) -> AccountInput {
        AccountInput {
            name: name.into(),
            kind,
            base_url: "http://localhost:8000/v1/".into(),
            key: Some("sk-or-abcdef1234".into()),
            use_in_orchestrator: true,
            models: vec![PinnedModel { id: "anthropic/claude-sonnet-5".into(), ..Default::default() }],
            prefs: Prefs::default(),
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
