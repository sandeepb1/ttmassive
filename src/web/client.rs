//! A client for Firecrawl's v2 API, and the shape its answers are given to an agent in: compact text, cut to a size
//! that does not flood a model's context, with the credits each call cost taken from the answer itself.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::error::{self, Kind, ProviderError};
use super::render;
use super::types::{Crawl, Fetched, MapEntry, Page, SearchHit, SearchOpts, SiteMap};
use crate::secrets::Account;

pub const DEFAULT_URL: &str = "https://api.firecrawl.dev";
/// Longest page text handed to an agent, in characters.
const MAX_PAGE: usize = 30_000;
/// Longest a whole answer may be.
const MAX_ANSWER: usize = 60_000;
/// Longest text taken from each page of a crawl.
const MAX_CRAWLED_PAGE: usize = 6_000;
/// Most pages one crawl call may fetch, whatever the agent asks for.
pub const MAX_CRAWL_PAGES: u32 = 20;
/// Pages crawled for a documentation snapshot.
pub const DOCS_PAGES: u32 = 10;
/// Longest text taken from each page of a documentation snapshot, and of the whole snapshot.
const MAX_DOC_PAGE: usize = 20_000;
const MAX_DOC_TOTAL: usize = 200_000;
const MAX_SEARCH: u32 = 10;
const MAX_MAP: usize = 100;
const CRAWL_POLL: Duration = Duration::from_secs(2);
const CRAWL_WAIT: Duration = Duration::from_secs(90);

pub struct Client {
    base: String,
    key: String,
    /// What one credit costs, in dollars, to price what a call used.
    usd_per_credit: f64,
    http: reqwest::Client,
    /// How long to wait between looks at a crawl.
    poll: Duration,
}

/// What a tool call gives back: text for the agent, and what it cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub text: String,
    pub credits: u64,
    /// What those credits cost, at the account's price per credit.
    pub usd: f64,
}

/// Credits left on an account, and what its plan includes.
#[derive(Debug, Clone, PartialEq)]
pub struct Balance {
    pub remaining: u64,
    pub plan: u64,
}

impl Client {
    pub fn new(account: &Account) -> Result<Client> {
        anyhow::ensure!(!account.key.is_empty(), "the Firecrawl account has no key");
        Ok(Client { usd_per_credit: account.credit_usd(), ..Client::with(&account.base_url, &account.key) })
    }

    pub fn with(base: &str, key: &str) -> Client {
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(120)).build().expect("http client");
        Client { base: base.trim_end_matches('/').to_owned(), key: key.to_owned(), usd_per_credit: super::types::Provider::Firecrawl.default_usd_per_credit(), http, poll: CRAWL_POLL }
    }

    fn output(&self, text: String, credits: u64) -> Output {
        Output { text, credits, usd: credits as f64 * self.usd_per_credit }
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
        let v = lenient_json(&text);
        if !status.is_success() {
            let why = v.as_ref().and_then(|v| v.get("error").or_else(|| v.get("message")).and_then(Value::as_str).map(String::from)).unwrap_or_else(|| text.chars().take(200).collect());
            return Err(ProviderError { kind: error::classify(status), retry_after: wait, message: format!("Firecrawl answered {status}: {why}") }.into());
        }
        v.ok_or_else(|| ProviderError::new(Kind::Network, "Firecrawl's answer was not JSON").into())
    }

    pub async fn credits(&self) -> Result<Balance> {
        let v = self.call(reqwest::Method::GET, "/v2/team/credit-usage", None).await?;
        let n = |k: &str| v.pointer(&format!("/data/{k}")).and_then(Value::as_u64);
        Ok(Balance { remaining: n("remainingCredits").context("no balance in the answer")?, plan: n("planCredits").unwrap_or(0) })
    }

    pub async fn search_hits(&self, query: &str, limit: u32, opts: SearchOpts) -> Result<Fetched<Vec<SearchHit>>> {
        let mut body = json!({"query": query, "limit": limit.clamp(1, MAX_SEARCH)});
        if let Some(r) = opts.recency {
            body["tbs"] = json!(r.tbs());
        }
        if opts.news {
            body["sources"] = json!(["news"]);
        }
        let v = self.call(reqwest::Method::POST, "/v2/search", Some(body)).await?;
        Ok(Fetched { value: parse_search(&v), credits: v.get("creditsUsed").and_then(Value::as_u64).unwrap_or(0) })
    }

    pub async fn scrape_page(&self, url: &str) -> Result<Fetched<Page>> {
        let v = self.call(reqwest::Method::POST, "/v2/scrape", Some(json!({"url": url, "formats": ["markdown"], "onlyMainContent": true}))).await?;
        Ok(Fetched { value: parse_page(&v["data"]), credits: v.pointer("/data/metadata/creditsUsed").and_then(Value::as_u64).unwrap_or(1) })
    }

    pub async fn map_site(&self, url: &str, limit: u32) -> Result<Fetched<SiteMap>> {
        let v = self.call(reqwest::Method::POST, "/v2/map", Some(json!({"url": url, "limit": limit.clamp(1, MAX_MAP as u32)}))).await?;
        // The answer does not say what it cost; a map is one credit.
        Ok(Fetched { value: parse_map(&v), credits: 1 })
    }

    /// Fetch up to `limit` pages starting at `url`, waiting for the crawl to finish (a bounded time).
    pub async fn crawl_site(&self, url: &str, limit: u32) -> Result<Fetched<Crawl>> {
        let limit = limit.clamp(1, MAX_CRAWL_PAGES);
        let started = self
            .call(reqwest::Method::POST, "/v2/crawl", Some(json!({"url": url, "limit": limit, "scrapeOptions": {"formats": ["markdown"], "onlyMainContent": true}})))
            .await?;
        let id = started.get("id").and_then(Value::as_str).context("the crawl had no id")?.to_owned();
        let deadline = tokio::time::Instant::now() + CRAWL_WAIT;
        loop {
            tokio::time::sleep(self.poll).await;
            let v = self.call(reqwest::Method::GET, &format!("/v2/crawl/{id}"), None).await?;
            let done = |v: &Value| Fetched { value: parse_crawl(v), credits: v.get("creditsUsed").and_then(Value::as_u64).unwrap_or(0) };
            match v.get("status").and_then(Value::as_str) {
                Some("completed") => return Ok(done(&v)),
                Some("failed" | "cancelled") => bail!("the crawl did not finish"),
                _ if tokio::time::Instant::now() >= deadline => {
                    // What it has got so far is still worth having; the rest is paid for only if it is fetched.
                    let _ = self.call(reqwest::Method::DELETE, &format!("/v2/crawl/{id}"), None).await;
                    return Ok(done(&v));
                }
                _ => {}
            }
        }
    }

    pub async fn search(&self, query: &str, limit: u32) -> Result<Output> {
        let f = self.search_hits(query, limit, SearchOpts::default()).await?;
        Ok(self.output(render::search(&f.value), f.credits))
    }

    pub async fn scrape(&self, url: &str) -> Result<Output> {
        let f = self.scrape_page(url).await?;
        Ok(self.output(render::page(&f.value, MAX_PAGE), f.credits))
    }

    pub async fn map(&self, url: &str, limit: u32) -> Result<Output> {
        let f = self.map_site(url, limit).await?;
        Ok(self.output(render::map(&f.value), f.credits))
    }

    pub async fn crawl(&self, url: &str, limit: u32) -> Result<Output> {
        let f = self.crawl_site(url, limit).await?;
        Ok(self.output(render::crawl(&f.value, MAX_CRAWLED_PAGE, MAX_ANSWER), f.credits))
    }

    /// A crawl kept in full, for reading later in files rather than handing straight to a model.
    pub async fn crawl_pages(&self, url: &str, limit: u32) -> Result<Output> {
        let f = self.crawl_site(url, limit).await?;
        Ok(self.output(render::crawl(&f.value, MAX_DOC_PAGE, MAX_DOC_TOTAL), f.credits))
    }
}

/// Parse JSON that may hold raw control characters inside strings, which Firecrawl's page text sometimes does and
/// which a strict parser refuses.
pub fn lenient_json(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(text) {
        return Some(v);
    }
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    for c in text.chars() {
        if in_string && !escaped && (c as u32) < 0x20 {
            out.push_str(&format!("\\u{:04x}", c as u32));
            continue;
        }
        match (in_string, escaped, c) {
            (true, false, '\\') => escaped = true,
            (true, true, _) => escaped = false,
            (_, false, '"') => in_string = !in_string,
            _ => {}
        }
        out.push(c);
    }
    serde_json::from_str(&out).ok()
}

fn text(r: &Value, k: &str) -> String {
    r.get(k).and_then(Value::as_str).unwrap_or("").trim().replace('\n', " ")
}

fn parse_search(v: &Value) -> Vec<SearchHit> {
    let web = v.pointer("/data/web").or_else(|| v.pointer("/data/news")).or_else(|| v.get("data")).and_then(Value::as_array).cloned().unwrap_or_default();
    web.iter()
        .map(|r| {
            // News items call the description a snippet, and carry a date.
            let snippet = if r.get("description").is_some() { text(r, "description") } else { text(r, "snippet") };
            let published = Some(text(r, "date")).filter(|d| !d.is_empty());
            SearchHit { title: text(r, "title"), url: text(r, "url"), snippet, published, score: None }
        })
        .collect()
}

fn parse_page(d: &Value) -> Page {
    let url = d.pointer("/metadata/sourceURL").or_else(|| d.pointer("/metadata/url")).and_then(Value::as_str).unwrap_or("");
    Page {
        url: url.to_owned(),
        title: d.pointer("/metadata/title").and_then(Value::as_str).unwrap_or("").trim().to_owned(),
        markdown: d.get("markdown").and_then(Value::as_str).unwrap_or("").trim().to_owned(),
    }
}

fn parse_map(v: &Value) -> SiteMap {
    let links = v.get("links").and_then(Value::as_array).cloned().unwrap_or_default();
    let pages = links
        .iter()
        .take(MAX_MAP)
        .filter_map(|l| {
            let url = l.get("url").and_then(Value::as_str).or_else(|| l.as_str())?.to_owned();
            let title = l.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned);
            Some(MapEntry { url, title })
        })
        .collect();
    SiteMap { pages }
}

fn parse_crawl(v: &Value) -> Crawl {
    Crawl { pages: v.get("data").and_then(Value::as_array).map(|a| a.iter().map(parse_page).collect()).unwrap_or_default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, extract::{Path, State}, http::HeaderMap, routing::{get, post}};
    use std::sync::{Arc, Mutex};

    #[test]
    fn json_with_raw_control_characters_in_strings_is_still_read() {
        let raw = "{\"status\":\"completed\",\"data\":[{\"markdown\":\"line one\nline two\ttabbed \\\" quote\"}]}";
        assert!(serde_json::from_str::<Value>(raw).is_err(), "a strict parser refuses it");
        let v = lenient_json(raw).expect("read leniently");
        assert_eq!(v["data"][0]["markdown"], "line one\nline two\ttabbed \" quote");
        assert_eq!(lenient_json("{\"a\":1}"), Some(json!({"a": 1})));
        assert_eq!(lenient_json("not json"), None);
        // A control character outside any string stays an error rather than being quietly accepted.
        assert!(lenient_json("{\"a\":\u{1}1}").is_none());
    }

    #[test]
    fn answers_are_read_into_the_shared_types() {
        let v = json!({"data": {"web": [{"url": "https://a.test", "title": " A ", "description": "first\nline"}]}});
        assert_eq!(parse_search(&v), vec![SearchHit { title: "A".into(), url: "https://a.test".into(), snippet: "first line".into(), published: None, score: None }]);
        // An older shape has the list directly under data.
        assert_eq!(parse_search(&json!({"data": [{"url": "u", "title": "t", "description": "d"}]}))[0].title, "t");
        assert!(parse_search(&json!({"data": {"web": []}})).is_empty());
        let page = parse_page(&json!({"markdown": " Hello ", "metadata": {"title": "Home", "sourceURL": "https://x.test/"}}));
        assert_eq!(page, Page { url: "https://x.test/".into(), title: "Home".into(), markdown: "Hello".into() });
        let site = parse_map(&json!({"links": [{"url": "https://x.test/a", "title": "A"}, {"url": "https://x.test/b", "title": " "}, "https://x.test/c"]}));
        assert_eq!(site.pages.iter().map(|e| (e.url.as_str(), e.title.as_deref())).collect::<Vec<_>>(), vec![("https://x.test/a", Some("A")), ("https://x.test/b", None), ("https://x.test/c", None)]);
        assert_eq!(parse_crawl(&json!({"data": [{"markdown": "m", "metadata": {"title": "T"}}]})).pages.len(), 1);
    }

    // ---- against a stand-in Firecrawl ----

    #[derive(Default)]
    struct Seen {
        auth: Mutex<Vec<String>>,
        polls: Mutex<u32>,
        bodies: Mutex<Vec<(String, Value)>>,
    }

    async fn serve() -> (String, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        let note = |seen: &Seen, headers: &HeaderMap, path: &str, body: Value| {
            seen.auth.lock().unwrap().push(headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned());
            seen.bodies.lock().unwrap().push((path.to_owned(), body));
        };
        let app = Router::new()
            .route("/v2/scrape", post({
                let note = note.clone();
                move |State(s): State<Arc<Seen>>, h: HeaderMap, axum::Json(b): axum::Json<Value>| async move {
                    note(&s, &h, "scrape", b);
                    axum::Json(json!({"success": true, "data": {"markdown": "Body text", "metadata": {"title": "T", "sourceURL": "https://x.test", "creditsUsed": 1}}}))
                }
            }))
            .route("/v2/search", post({
                let note = note.clone();
                move |State(s): State<Arc<Seen>>, h: HeaderMap, axum::Json(b): axum::Json<Value>| async move {
                    note(&s, &h, "search", b);
                    axum::Json(json!({"success": true, "creditsUsed": 2, "data": {"web": [{"url": "https://a.test", "title": "A", "description": "d"}]}}))
                }
            }))
            .route("/v2/crawl", post({
                let note = note.clone();
                move |State(s): State<Arc<Seen>>, h: HeaderMap, axum::Json(b): axum::Json<Value>| async move {
                    note(&s, &h, "crawl", b);
                    axum::Json(json!({"success": true, "id": "job1"}))
                }
            }))
            .route("/v2/crawl/{id}", get(|State(s): State<Arc<Seen>>, Path(_id): Path<String>| async move {
                let mut n = s.polls.lock().unwrap();
                *n += 1;
                // The second look finds it done, with a raw newline in the page text as Firecrawl really sends.
                let body = if *n < 2 { "{\"status\":\"scraping\",\"data\":[]}".to_string() } else { "{\"status\":\"completed\",\"creditsUsed\":2,\"data\":[{\"markdown\":\"a\nb\",\"metadata\":{\"title\":\"P\",\"sourceURL\":\"https://x.test/p\"}}]}".to_string() };
                ([("content-type", "application/json")], body)
            }))
            .route("/v2/team/credit-usage", get(|| async { axum::Json(json!({"success": true, "data": {"remainingCredits": 873, "planCredits": 1000}})) }))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, seen)
    }

    #[tokio::test]
    async fn scrape_and_search_send_the_key_and_report_what_they_cost() {
        let (base, seen) = serve().await;
        let c = Client::with(&format!("{base}/"), "fc-key");
        let s = c.scrape("https://x.test").await.unwrap();
        assert_eq!((s.text.as_str(), s.credits), ("# T\nhttps://x.test\n\nBody text", 1));
        let r = c.search("rust", 99).await.unwrap();
        assert_eq!(r.credits, 2);
        assert!(r.text.contains("1. A — https://a.test"));
        assert!(seen.auth.lock().unwrap().iter().all(|a| a == "Bearer fc-key"));
        let bodies = seen.bodies.lock().unwrap();
        assert_eq!(bodies[1].1["limit"], 10, "an agent cannot ask for more than the cap");
        assert_eq!(bodies[0].1["onlyMainContent"], true);
    }

    #[tokio::test]
    async fn a_crawl_is_waited_for_and_read_despite_raw_control_characters() {
        let (base, seen) = serve().await;
        let mut c = Client::with(&base, "fc-key");
        c.poll = Duration::from_millis(10);
        let out = c.crawl("https://x.test", 500).await.unwrap();
        assert!(out.text.starts_with("1 page(s):") && out.text.contains("# P\nhttps://x.test/p\n\na\nb"), "{}", out.text);
        assert_eq!(out.credits, 2);
        assert_eq!(*seen.polls.lock().unwrap(), 2);
        assert_eq!(seen.bodies.lock().unwrap()[0].1["limit"], MAX_CRAWL_PAGES, "the page cap holds whatever was asked");
        assert_eq!(c.credits().await.unwrap(), Balance { remaining: 873, plan: 1000 });
    }

    #[tokio::test]
    async fn an_error_from_the_service_is_reported_with_its_reason() {
        let app = Router::new().route("/v2/scrape", post(|| async { (axum::http::StatusCode::PAYMENT_REQUIRED, axum::Json(json!({"error": "Insufficient credits"}))) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let err = Client::with(&base, "k").scrape("https://x.test").await.unwrap_err().to_string();
        assert!(err.contains("402") && err.contains("Insufficient credits"), "{err}");
    }
}
