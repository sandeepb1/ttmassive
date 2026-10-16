//! OpenRouter's model list (public, so no key is needed), trimmed to what pickers need and kept
//! for an hour in memory and on disk. Also asks an account for its balance, and an
//! OpenAI-compatible endpoint for its model ids.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::secrets::{Account, Kind};

const FILE: &str = "openrouter-models.json";
const TTL: Duration = Duration::from_secs(3600);
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_DESCRIPTION: usize = 240;

/// One model as the page needs it. Prices are dollars per million tokens; `None` is unknown (a
/// router entry such as `openrouter/auto` has a negative price).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    /// Unix seconds the model was added.
    pub created: i64,
    pub description: String,
    pub context: u64,
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
    /// Per million cached input tokens, when the model has a cache price.
    pub price_cache_read: Option<f64>,
    pub tools: bool,
    pub reasoning: bool,
    pub structured: bool,
    pub image_in: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    /// Unix seconds this list was fetched.
    pub fetched: i64,
    pub models: Vec<Model>,
}

/// What an account says about itself, for the Settings list.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    /// OpenRouter only.
    pub usage: Option<f64>,
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub free_tier: Option<bool>,
    /// OpenRouter only: spend so far today and this month, when it says.
    pub usage_daily: Option<f64>,
    pub usage_monthly: Option<f64>,
    /// OpenAI-compatible only: the endpoint's own model ids.
    pub models: Vec<String>,
    /// What `usage`, `limit` and `remaining` count, when not dollars: Firecrawl's credits.
    pub unit: Option<&'static str>,
}

static MEMORY: Mutex<Option<Arc<Catalog>>> = Mutex::new(None);

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn fresh(c: &Catalog) -> bool {
    !c.models.is_empty() && (now() - c.fetched).unsigned_abs() < TTL.as_secs()
}

/// Dollars per token as OpenRouter writes them (a string) to dollars per million tokens.
fn per_million(v: Option<&Value>) -> Option<f64> {
    let p: f64 = match v? {
        Value::String(s) => s.trim().parse().ok()?,
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    (p.is_finite() && p >= 0.0).then(|| (p * 1.0e12).round() / 1.0e6)
}

fn parse_model(v: &Value) -> Option<Model> {
    let id = v.get("id")?.as_str()?.to_owned();
    let params: Vec<&str> = v.get("supported_parameters").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let has = |p: &str| params.contains(&p);
    let pricing = v.get("pricing");
    let image_in = v
        .pointer("/architecture/input_modalities")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|m| m.as_str() == Some("image")));
    let description: String = v.get("description").and_then(Value::as_str).unwrap_or("").chars().take(MAX_DESCRIPTION).collect();
    Some(Model {
        name: v.get("name").and_then(Value::as_str).unwrap_or(&id).to_owned(),
        created: v.get("created").and_then(Value::as_i64).unwrap_or(0),
        description,
        context: v.get("context_length").and_then(Value::as_u64).unwrap_or(0),
        price_in: per_million(pricing.and_then(|p| p.get("prompt"))),
        price_out: per_million(pricing.and_then(|p| p.get("completion"))),
        price_cache_read: per_million(pricing.and_then(|p| p.get("input_cache_read"))),
        tools: has("tools"),
        reasoning: has("reasoning") || has("reasoning_effort"),
        structured: has("structured_outputs") || has("response_format"),
        image_in,
        id,
    })
}

/// Parse the body of `GET /models`.
pub fn parse(body: &Value) -> Vec<Model> {
    body.get("data").and_then(Value::as_array).map(|a| a.iter().filter_map(parse_model).collect()).unwrap_or_default()
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(TIMEOUT).build()?)
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

async fn fetch(base: &str) -> Result<Catalog> {
    let res = client()?.get(format!("{base}/models")).send().await.context("asking OpenRouter for its models")?;
    if !res.status().is_success() {
        bail!("OpenRouter answered {}", res.status());
    }
    let models = parse(&res.json::<Value>().await.context("reading OpenRouter's model list")?);
    if models.is_empty() {
        bail!("OpenRouter sent no models");
    }
    Ok(Catalog { fetched: now(), models })
}

/// The catalog, from memory, then disk, then OpenRouter. A failed fetch falls back to a stale copy
/// rather than leave the pickers empty. `force` skips the caches.
pub async fn get(data_dir: &Path, force: bool) -> Result<Arc<Catalog>> {
    if !force {
        if let Some(c) = MEMORY.lock().unwrap().clone().filter(|c| fresh(c)) {
            return Ok(c);
        }
        if let Some(c) = read_disk(data_dir).await.filter(fresh) {
            let c = Arc::new(c);
            *MEMORY.lock().unwrap() = Some(c.clone());
            return Ok(c);
        }
    }
    match fetch(crate::secrets::OPENROUTER_URL).await {
        Ok(c) => {
            if let Err(e) = write_disk(data_dir, &c).await {
                tracing::warn!("saving the OpenRouter catalog: {e:#}");
            }
            let c = Arc::new(c);
            *MEMORY.lock().unwrap() = Some(c.clone());
            Ok(c)
        }
        Err(e) => {
            let held = MEMORY.lock().unwrap().clone();
            let stale = match held {
                Some(c) => Some(c),
                None => read_disk(data_dir).await.map(Arc::new),
            };
            stale.filter(|c| !c.models.is_empty()).ok_or(e)
        }
    }
}

async fn read_disk(data_dir: &Path) -> Option<Catalog> {
    serde_json::from_slice(&tokio::fs::read(path(data_dir)).await.ok()?).ok()
}

async fn write_disk(data_dir: &Path, c: &Catalog) -> Result<()> {
    let p = path(data_dir);
    let tmp = p.with_extension("json.tmp");
    tokio::fs::write(&tmp, serde_json::to_vec(c)?).await?;
    tokio::fs::rename(&tmp, &p).await?;
    Ok(())
}

/// Model ids from an OpenAI-compatible `/models` answer. Gateways list patterns such as `*` or
/// `anthropic/*` to say "anything under this prefix"; those are not models.
pub fn endpoint_models(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = body
        .get("data")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|m| m.get("id").and_then(Value::as_str)).filter(|id| !id.is_empty() && !id.contains('*')).map(str::to_owned).collect())
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    ids
}

/// Ask an account about itself: the balance for OpenRouter, the model ids for anything else.
pub async fn remote(account: &Account) -> Result<Remote> {
    if account.key.is_empty() {
        bail!("this account has no key");
    }
    let http = client()?;
    let get = |url: String| http.get(url).bearer_auth(&account.key).send();
    match account.kind {
        Kind::Openrouter => {
            let res = get(format!("{}/key", account.base_url)).await.context("asking OpenRouter about the key")?;
            if !res.status().is_success() {
                bail!("OpenRouter answered {}", res.status());
            }
            let v: Value = res.json().await?;
            let d = v.get("data").unwrap_or(&Value::Null);
            let num = |k: &str| d.get(k).and_then(Value::as_f64);
            Ok(Remote { usage: num("usage"), limit: num("limit"), remaining: num("limit_remaining"), free_tier: d.get("is_free_tier").and_then(Value::as_bool), usage_daily: num("usage_daily"), usage_monthly: num("usage_monthly"), models: Vec::new(), unit: None })
        }
        Kind::Firecrawl => {
            let b = crate::web::client::Client::new(account)?.credits().await?;
            Ok(Remote { usage: Some(b.plan.saturating_sub(b.remaining) as f64), limit: Some(b.plan as f64), remaining: Some(b.remaining as f64), unit: Some("credits"), ..Default::default() })
        }
        Kind::OpenaiCompatible => {
            let res = get(format!("{}/models", account.base_url)).await.context("asking the endpoint for its models")?;
            if !res.status().is_success() {
                bail!("the endpoint answered {}", res.status());
            }
            Ok(Remote { models: endpoint_models(&res.json::<Value>().await?), ..Default::default() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prices_become_dollars_per_million() {
        assert_eq!(per_million(Some(&json!("0.0000003"))), Some(0.3));
        assert_eq!(per_million(Some(&json!("0.0000025"))), Some(2.5));
        assert_eq!(per_million(Some(&json!("0"))), Some(0.0));
        assert_eq!(per_million(Some(&json!("-1"))), None, "router entries carry a negative price");
        assert_eq!(per_million(Some(&json!("n/a"))), None);
        assert_eq!(per_million(None), None);
    }

    #[test]
    fn parses_a_model() {
        let body = json!({"data": [{
            "id": "moondream/moondream3-preview", "name": "Moondream 3", "created": 1792102385,
            "description": "x".repeat(500), "context_length": 32000,
            "architecture": {"input_modalities": ["text", "image"]},
            "pricing": {"prompt": "0.0000003", "completion": "0.0000025", "input_cache_read": "0.00000003"},
            "supported_parameters": ["tools", "reasoning", "structured_outputs", "temperature"]
        }, {"name": "no id"}]});
        let m = parse(&body);
        assert_eq!(m.len(), 1, "an entry without an id is dropped");
        let m = &m[0];
        assert_eq!((m.price_in, m.price_out, m.price_cache_read), (Some(0.3), Some(2.5), Some(0.03)));
        assert!(m.tools && m.reasoning && m.structured && m.image_in);
        assert_eq!(m.context, 32000);
        assert_eq!(m.description.chars().count(), MAX_DESCRIPTION);
    }

    #[test]
    fn a_bare_model_has_no_capabilities() {
        let m = &parse(&json!({"data": [{"id": "a/b"}]}))[0];
        assert_eq!((m.name.as_str(), m.created, m.context), ("a/b", 0, 0));
        assert!(!m.tools && !m.reasoning && !m.structured && !m.image_in);
        assert_eq!(m.price_in, None);
    }

    #[test]
    fn endpoint_wildcards_are_not_models() {
        let body = json!({"data": [{"id": "*"}, {"id": "xai/*"}, {"id": "b/two"}, {"id": "a/one"}, {"id": "a/one"}, {"id": ""}]});
        assert_eq!(endpoint_models(&body), vec!["a/one", "b/two"]);
        assert!(endpoint_models(&json!({})).is_empty());
    }

    #[test]
    fn freshness_follows_the_ttl() {
        let old = Catalog { fetched: now() - 7200, models: parse(&json!({"data": [{"id": "a/b"}]})) };
        assert!(!fresh(&old));
        assert!(fresh(&Catalog { fetched: now() - 60, ..old.clone() }));
        assert!(!fresh(&Catalog { fetched: now(), models: Vec::new() }), "an empty list is never fresh");
    }

    #[tokio::test]
    async fn disk_copy_round_trips() {
        let dir = std::env::temp_dir().join(format!("ttm-catalog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_disk(&dir).await.is_none());
        let c = Catalog { fetched: now(), models: parse(&json!({"data": [{"id": "a/b", "pricing": {"prompt": "0.000001"}}]})) };
        write_disk(&dir, &c).await.unwrap();
        let back = read_disk(&dir).await.unwrap();
        assert_eq!(back.models, c.models);
        assert!(fresh(&back));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
