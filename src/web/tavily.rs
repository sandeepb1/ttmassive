//! A client for Tavily's API, giving its answers in the shared shapes. Tavily takes a bearer key, answers errors as
//! `{"detail": {"error": …}}`, and says what a call cost when asked to (`include_usage`).

use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use super::client::MAX_CRAWL_PAGES;
use super::error::{self, Kind, ProviderError};
use super::types::{Crawl, Fetched, MapEntry, Page, SearchHit, SearchOpts, SiteMap};
use crate::secrets::Account;

pub const DEFAULT_URL: &str = "https://api.tavily.com";
const MAX_SEARCH: u32 = 10;
const MAX_MAP: u32 = 100;
/// How long Tavily is told it may spend on a crawl; the connection waits longer than this.
const CRAWL_SECONDS: u32 = 90;

pub struct Client {
    base: String,
    key: String,
    http: reqwest::Client,
}

/// What an account has used, and what it may.
#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub used: u64,
    pub limit: Option<u64>,
}

impl Client {
    pub fn new(account: &Account) -> Result<Client> {
        anyhow::ensure!(!account.key.is_empty(), "the Tavily account has no key");
        Ok(Client::with(&account.base_url, &account.key))
    }

    pub fn with(base: &str, key: &str) -> Client {
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(120)).build().expect("http client");
        Client { base: base.trim_end_matches('/').to_owned(), key: key.to_owned(), http }
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
        let mut req = self.http.request(method, format!("{}{path}", self.base)).bearer_auth(&self.key);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.map_err(|e| ProviderError::new(Kind::Network, format!("could not reach {}: {e}", self.base)))?;
        let status = res.status();
        let wait = error::retry_after(res.headers());
        let text = res.text().await.unwrap_or_default();
        let v = serde_json::from_str::<Value>(&text).ok();
        if !status.is_success() {
            let why = v.as_ref().and_then(reason).unwrap_or_else(|| text.chars().take(200).collect());
            let kind = match status.as_u16() {
                // Tavily's own codes for a key or plan limit, and for the pay-as-you-go limit.
                432 | 433 => Kind::Exhausted,
                403 if why.to_lowercase().contains("support") => Kind::Unsupported,
                _ => error::classify(status),
            };
            return Err(ProviderError { kind, retry_after: wait, message: format!("Tavily answered {status}: {why}") }.into());
        }
        v.ok_or_else(|| ProviderError::new(Kind::Network, "Tavily's answer was not JSON").into())
    }

    pub async fn usage(&self) -> Result<Usage> {
        let v = self.call(reqwest::Method::GET, "/usage", None).await?;
        // A plan's figures are the account's; a key may have a limit of its own, which comes first.
        let n = |p: &str| v.pointer(p).and_then(Value::as_f64).map(|n| n as u64);
        let (used, limit) = match n("/key/limit") {
            Some(l) => (n("/key/usage"), Some(l)),
            None => (n("/account/plan_usage").or_else(|| n("/key/usage")), n("/account/plan_limit")),
        };
        Ok(Usage { used: used.ok_or_else(|| ProviderError::new(Kind::Network, "no usage in Tavily's answer"))?, limit })
    }

    pub async fn search_hits(&self, query: &str, limit: u32, opts: SearchOpts) -> Result<Fetched<Vec<SearchHit>>> {
        let mut body = json!({"query": query, "max_results": limit.clamp(1, MAX_SEARCH), "search_depth": "basic", "include_usage": true,
                              "topic": if opts.news { "news" } else { "general" }});
        if let Some(r) = opts.recency {
            body["time_range"] = json!(r.as_str());
        }
        let v = self.call(reqwest::Method::POST, "/search", Some(body)).await?;
        let hits = v.get("results").and_then(Value::as_array).map(|a| a.iter().map(hit).collect()).unwrap_or_default();
        // A basic search is one credit, which the answer confirms.
        Ok(Fetched { value: hits, credits: credits(&v).unwrap_or(1) })
    }

    pub async fn scrape_page(&self, url: &str) -> Result<Fetched<Page>> {
        let body = json!({"urls": [url], "format": "markdown", "extract_depth": "basic", "include_usage": true});
        let v = self.call(reqwest::Method::POST, "/extract", Some(body)).await?;
        let page = v.pointer("/results/0").map(page).unwrap_or_else(|| Page { url: url.to_owned(), title: String::new(), markdown: String::new() });
        // Failed addresses are not charged; the answer may round a lone page down to nothing, but it was paid for.
        let spent = if page.markdown.is_empty() { credits(&v).unwrap_or(0) } else { credits(&v).filter(|c| *c > 0).unwrap_or(1) };
        Ok(Fetched { value: page, credits: spent })
    }

    pub async fn map_site(&self, url: &str, limit: u32) -> Result<Fetched<SiteMap>> {
        let body = json!({"url": url, "limit": limit.clamp(1, MAX_MAP), "include_usage": true});
        let v = self.call(reqwest::Method::POST, "/map", Some(body)).await?;
        let pages: Vec<MapEntry> = v
            .get("results")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(|u| MapEntry { url: u.to_owned(), title: None }).collect())
            .unwrap_or_default();
        // A map is a credit for every ten pages.
        let estimate = (pages.len() as u64).div_ceil(10).max(1);
        Ok(Fetched { credits: credits(&v).filter(|c| *c > 0).unwrap_or(estimate), value: SiteMap { pages } })
    }

    pub async fn crawl_site(&self, url: &str, limit: u32) -> Result<Fetched<Crawl>> {
        let body = json!({"url": url, "limit": limit.clamp(1, MAX_CRAWL_PAGES), "extract_depth": "basic", "format": "markdown", "include_usage": true, "timeout": CRAWL_SECONDS});
        let v = self.call(reqwest::Method::POST, "/crawl", Some(body)).await?;
        let pages: Vec<Page> = v.get("results").and_then(Value::as_array).map(|a| a.iter().map(page).collect()).unwrap_or_default();
        // The map's cost, and a credit for every five pages read.
        let n = pages.len() as u64;
        let estimate = (n.div_ceil(10) + n.div_ceil(5)).max(1);
        Ok(Fetched { credits: credits(&v).filter(|c| *c > 0).unwrap_or(estimate), value: Crawl { pages } })
    }
}

/// Why Tavily refused, from whichever of its shapes the answer used.
fn reason(v: &Value) -> Option<String> {
    let d = v.get("detail")?;
    d.pointer("/error").or_else(|| d.as_str().map(|_| d)).or_else(|| d.pointer("/0/msg")).and_then(Value::as_str).map(str::to_owned)
}

/// Credits the answer says it used, if it says.
fn credits(v: &Value) -> Option<u64> {
    v.pointer("/usage/credits").and_then(Value::as_f64).map(|c| c.ceil() as u64)
}

fn text(r: &Value, k: &str) -> String {
    r.get(k).and_then(Value::as_str).unwrap_or("").trim().replace('\n', " ")
}

fn hit(r: &Value) -> SearchHit {
    SearchHit {
        title: text(r, "title"),
        url: text(r, "url"),
        snippet: text(r, "content"),
        published: Some(text(r, "published_date")).filter(|d| !d.is_empty()),
        score: r.get("score").and_then(Value::as_f64),
    }
}

fn page(r: &Value) -> Page {
    // Tavily gives a page's text but not its title.
    Page { url: text(r, "url"), title: String::new(), markdown: r.get("raw_content").and_then(Value::as_str).unwrap_or("").trim().to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::types::Recency;
    use crate::web::testutil::{ok, serve};

    #[tokio::test]
    async fn a_search_sends_the_key_and_options_and_reads_hits_and_cost() {
        let (base, seen) = serve(vec![ok("/search", json!({"usage": {"credits": 1}, "results": [
            {"title": " T ", "url": "https://a.test", "content": "line\none", "score": 0.9, "published_date": "2026-10-01"},
            {"title": "U", "url": "https://b.test", "content": "", "score": 0.5}]}))])
        .await;
        let c = Client::with(&format!("{base}/"), "tvly-k");
        let f = c.search_hits("rust", 99, SearchOpts { recency: Some(Recency::Week), news: true }).await.unwrap();
        assert_eq!(f.credits, 1);
        assert_eq!(f.value[0], SearchHit { title: "T".into(), url: "https://a.test".into(), snippet: "line one".into(), published: Some("2026-10-01".into()), score: Some(0.9) });
        assert_eq!(f.value[1].published, None);
        assert_eq!(seen.auth.lock().unwrap()[0], "Bearer tvly-k");
        let b = seen.bodies.lock().unwrap()[0].1.clone();
        assert_eq!((b["max_results"].clone(), b["topic"].clone(), b["time_range"].clone(), b["include_usage"].clone()), (json!(10), json!("news"), json!("week"), json!(true)));
        c.search_hits("rust", 3, SearchOpts::default()).await.unwrap();
        let b = seen.bodies.lock().unwrap()[1].1.clone();
        assert!(b["topic"] == "general" && b.get("time_range").is_none());
    }

    #[tokio::test]
    async fn a_page_a_map_and_a_crawl_are_read_and_priced() {
        let (base, seen) = serve(vec![
            ok("/extract", json!({"usage": {"credits": 0}, "results": [{"url": "https://x.test", "raw_content": " Body "}], "failed_results": []})),
            ok("/map", json!({"results": ["https://x.test/a", "https://x.test/b"]})),
            ok("/crawl", json!({"usage": {"credits": 3}, "results": [{"url": "https://x.test/a", "raw_content": "one"}, {"url": "https://x.test/b", "raw_content": "two"}]})),
        ])
        .await;
        let c = Client::with(&base, "k");
        let p = c.scrape_page("https://x.test").await.unwrap();
        assert_eq!((p.value, p.credits), (Page { url: "https://x.test".into(), title: String::new(), markdown: "Body".into() }, 1), "a page that was read is at least a credit, whatever the answer rounds to");
        let m = c.map_site("https://x.test", 500).await.unwrap();
        assert_eq!((m.value.pages.len(), m.credits), (2, 1), "an unreported map is a credit per ten pages");
        let cr = c.crawl_site("https://x.test", 500).await.unwrap();
        assert_eq!((cr.value.pages.len(), cr.credits), (2, 3));
        let bodies = seen.bodies.lock().unwrap();
        assert_eq!(bodies[0].1["urls"], json!(["https://x.test"]));
        assert_eq!(bodies[1].1["limit"], 100, "a map is capped");
        assert_eq!(bodies[2].1["limit"], MAX_CRAWL_PAGES, "and so is a crawl");
    }

    #[tokio::test]
    async fn a_page_that_could_not_be_read_comes_back_empty_and_free() {
        let (base, _) = serve(vec![ok("/extract", json!({"results": [], "failed_results": [{"url": "https://x.test", "error": "timed out"}]}))]).await;
        let f = Client::with(&base, "k").scrape_page("https://x.test").await.unwrap();
        assert!(f.value.markdown.is_empty() && f.credits == 0);
        assert_eq!(f.value.url, "https://x.test");
    }

    #[tokio::test]
    async fn usage_is_the_keys_limit_when_it_has_one_and_the_plans_when_not() {
        let (base, _) = serve(vec![ok("/usage", json!({"key": {"usage": 12, "limit": 100}, "account": {"plan_usage": 40, "plan_limit": 1000}}))]).await;
        assert_eq!(Client::with(&base, "k").usage().await.unwrap(), Usage { used: 12, limit: Some(100) });
        let (base, _) = serve(vec![ok("/usage", json!({"key": {"usage": 12, "limit": null}, "account": {"plan_usage": 40, "plan_limit": 1000}}))]).await;
        assert_eq!(Client::with(&base, "k").usage().await.unwrap(), Usage { used: 40, limit: Some(1000) });
    }

    #[tokio::test]
    async fn failures_are_told_apart_by_status_and_reason() {
        let kind = |status: u16, headers: Vec<(&'static str, &'static str)>, body: Value| async move {
            let (base, _) = serve(vec![("/search", status, headers, body)]).await;
            let e = Client::with(&base, "k").search_hits("x", 5, SearchOpts::default()).await.unwrap_err();
            let pe = e.downcast::<ProviderError>().unwrap();
            (pe.kind, pe.retry_after, pe.message)
        };
        assert_eq!(kind(432, vec![], json!({"detail": {"error": "This request exceeds your plan's set usage limit."}})).await.0, Kind::Exhausted);
        assert_eq!(kind(433, vec![], json!({"detail": {"error": "pay-as-you-go limit"}})).await.0, Kind::Exhausted);
        let (k, wait, msg) = kind(429, vec![("retry-after", "7")], json!({"detail": {"error": "Too many requests"}})).await;
        assert_eq!((k, wait), (Kind::RateLimited, Some(Duration::from_secs(7))));
        assert!(msg.contains("429") && msg.contains("Too many requests"), "{msg}");
        assert_eq!(kind(401, vec![], json!({"detail": {"error": "Unauthorized: missing or invalid API key."}})).await.0, Kind::BadKey);
        assert_eq!(kind(403, vec![], json!({"detail": {"error": "URL is not supported"}})).await.0, Kind::Unsupported);
        assert_eq!(kind(403, vec![], json!({"detail": {"error": "Forbidden"}})).await.0, Kind::BadKey);
        assert_eq!(kind(500, vec![], json!({"detail": {"error": "oops"}})).await.0, Kind::Server);
        let (k, _, msg) = kind(422, vec![], json!({"detail": [{"type": "missing", "loc": ["body", "query"], "msg": "Field required"}]})).await;
        assert_eq!(k, Kind::Rejected);
        assert!(msg.contains("Field required"), "{msg}");
        assert_eq!(kind(400, vec![], json!({"detail": {"error": "bad"}})).await.0, Kind::Rejected);
    }

    /// Against the real service, with `TAVILY_API_KEY` set: `cargo test live_tavily -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs TAVILY_API_KEY and spends a few credits"]
    async fn live_tavily_answers_every_tool_in_the_shared_shapes() {
        let c = Client::with(DEFAULT_URL, &std::env::var("TAVILY_API_KEY").expect("TAVILY_API_KEY"));
        let before = c.usage().await.unwrap();
        let s = c.search_hits("rust programming language", 3, SearchOpts::default()).await.unwrap();
        println!("search: {} hits, {} credits, first {:?}", s.value.len(), s.credits, s.value.first());
        assert!(!s.value.is_empty() && s.value.iter().all(|h| h.url.starts_with("http")));
        let n = c.search_hits("rust release", 3, SearchOpts { recency: Some(Recency::Month), news: true }).await.unwrap();
        println!("news: {} hits, {} credits, first {:?}", n.value.len(), n.credits, n.value.first());
        let p = c.scrape_page("https://example.com").await.unwrap();
        println!("scrape: {} chars, {} credits", p.value.markdown.len(), p.credits);
        assert!(p.value.markdown.contains("This domain is for use"));
        let m = c.map_site("https://docs.tavily.com", 10).await.unwrap();
        println!("map: {} pages, {} credits", m.value.pages.len(), m.credits);
        assert!(!m.value.pages.is_empty());
        let cr = c.crawl_site("https://docs.tavily.com", 2).await.unwrap();
        println!("crawl: {} pages, {} credits", cr.value.pages.len(), cr.credits);
        assert!(!cr.value.pages.is_empty() && cr.value.pages.len() <= 2);
        let after = c.usage().await.unwrap();
        println!("usage: {before:?} -> {after:?}");
    }
}
