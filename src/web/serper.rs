//! A client for Serper's Google search API, giving its answers in the shared shapes. Serper only searches: pages,
//! maps and crawls are other services' work. It takes the key in `X-API-KEY`, says what each call cost in the answer,
//! and reports its balance at `/account`.

use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use super::error::{self, Kind, ProviderError};
use super::types::{Fetched, SearchHit, SearchOpts};
use crate::secrets::Account;

pub const DEFAULT_URL: &str = "https://google.serper.dev";
/// Serper answers a search with ten results at most for one credit; more costs more.
const MAX_SEARCH: u32 = 10;

pub struct Client {
    base: String,
    key: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(account: &Account) -> Result<Client> {
        anyhow::ensure!(!account.key.is_empty(), "the Serper account has no key");
        Ok(Client::with(&account.base_url, &account.key))
    }

    pub fn with(base: &str, key: &str) -> Client {
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(30)).build().expect("http client");
        Client { base: base.trim_end_matches('/').to_owned(), key: key.to_owned(), http }
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
        let mut req = self.http.request(method, format!("{}{path}", self.base)).header("X-API-KEY", &self.key);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.map_err(|e| ProviderError::new(Kind::Network, format!("could not reach {}: {e}", self.base)))?;
        let status = res.status();
        let wait = error::retry_after(res.headers());
        let text = res.text().await.unwrap_or_default();
        let v = serde_json::from_str::<Value>(&text).ok();
        if !status.is_success() {
            let why = v.as_ref().and_then(|v| v.get("message").and_then(Value::as_str).map(String::from)).unwrap_or_else(|| text.chars().take(200).collect());
            let kind = match error::classify(status) {
                // Serper does not document how it says a balance is spent, so any refusal that talks of credits is taken as one.
                Kind::Rejected | Kind::BadKey if why.to_lowercase().contains("credit") => Kind::Exhausted,
                k => k,
            };
            return Err(ProviderError { kind, retry_after: wait, message: format!("Serper answered {status}: {why}") }.into());
        }
        v.ok_or_else(|| ProviderError::new(Kind::Network, "Serper's answer was not JSON").into())
    }

    /// Credits left on the account.
    pub async fn balance(&self) -> Result<u64> {
        let v = self.call(reqwest::Method::GET, "/account", None).await?;
        v.get("balance").and_then(Value::as_u64).ok_or_else(|| ProviderError::new(Kind::Network, "no balance in Serper's answer").into())
    }

    pub async fn search_hits(&self, query: &str, limit: u32, opts: SearchOpts) -> Result<Fetched<Vec<SearchHit>>> {
        let limit = limit.clamp(1, MAX_SEARCH);
        let mut body = json!({"q": query, "num": limit});
        if let Some(r) = opts.recency {
            body["tbs"] = json!(r.tbs());
        }
        let (path, list) = if opts.news { ("/news", "news") } else { ("/search", "organic") };
        let v = self.call(reqwest::Method::POST, path, Some(body)).await?;
        // News ignores the count and gives ten, so the limit is kept here.
        let hits = v.get(list).and_then(Value::as_array).map(|a| a.iter().take(limit as usize).map(hit).collect()).unwrap_or_default();
        Ok(Fetched { value: hits, credits: v.get("credits").and_then(Value::as_u64).unwrap_or(1) })
    }
}

fn text(r: &Value, k: &str) -> String {
    r.get(k).and_then(Value::as_str).unwrap_or("").trim().replace('\n', " ")
}

fn hit(r: &Value) -> SearchHit {
    SearchHit {
        title: text(r, "title"),
        url: text(r, "link"),
        snippet: text(r, "snippet"),
        // "2 days ago", as Google gives it.
        published: Some(text(r, "date")).filter(|d| !d.is_empty()),
        score: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::testutil::{ok, serve};
    use crate::web::types::Recency;

    #[tokio::test]
    async fn a_search_sends_the_key_and_options_and_reads_hits_and_cost() {
        let (base, seen) = serve(vec![ok("/search", json!({"credits": 1, "organic": [
            {"title": " T ", "link": "https://a.test", "snippet": "line\none", "date": "2 days ago", "position": 1},
            {"title": "U", "link": "https://b.test", "snippet": "s", "position": 2}]}))])
        .await;
        let c = Client::with(&format!("{base}/"), "sk");
        let f = c.search_hits("rust", 99, SearchOpts { recency: Some(Recency::Month), news: false }).await.unwrap();
        assert_eq!(f.credits, 1);
        assert_eq!(f.value[0], SearchHit { title: "T".into(), url: "https://a.test".into(), snippet: "line one".into(), published: Some("2 days ago".into()), score: None });
        assert_eq!(f.value[1].published, None);
        assert_eq!(seen.auth.lock().unwrap()[0], "sk");
        let b = seen.bodies.lock().unwrap()[0].1.clone();
        assert_eq!((b["q"].clone(), b["num"].clone(), b["tbs"].clone()), (json!("rust"), json!(10), json!("qdr:m")));
        c.search_hits("rust", 3, SearchOpts::default()).await.unwrap();
        assert!(seen.bodies.lock().unwrap()[1].1.get("tbs").is_none());
    }

    #[tokio::test]
    async fn news_comes_from_its_own_endpoint_and_is_cut_to_the_limit() {
        let items: Vec<Value> = (0..10).map(|i| json!({"title": format!("N{i}"), "link": format!("https://n.test/{i}"), "snippet": "s", "date": "1 day ago", "source": "Site"})).collect();
        let (base, seen) = serve(vec![ok("/news", json!({"credits": 1, "news": items}))]).await;
        let f = Client::with(&base, "sk").search_hits("rust", 3, SearchOpts { recency: Some(Recency::Week), news: true }).await.unwrap();
        assert_eq!(f.value.iter().map(|h| h.title.as_str()).collect::<Vec<_>>(), ["N0", "N1", "N2"]);
        assert_eq!(seen.bodies.lock().unwrap()[0].1["tbs"], "qdr:w");
    }

    #[tokio::test]
    async fn no_hits_is_an_empty_list_and_a_call_still_costs_what_it_says() {
        let (base, _) = serve(vec![ok("/search", json!({"credits": 2}))]).await;
        let f = Client::with(&base, "sk").search_hits("x", 5, SearchOpts::default()).await.unwrap();
        assert!(f.value.is_empty());
        assert_eq!(f.credits, 2);
        let (base, _) = serve(vec![ok("/search", json!({"organic": []}))]).await;
        assert_eq!(Client::with(&base, "sk").search_hits("x", 5, SearchOpts::default()).await.unwrap().credits, 1, "an answer that does not say costs the usual one");
    }

    #[tokio::test]
    async fn the_balance_is_read_from_the_account() {
        let (base, seen) = serve(vec![ok("/account", json!({"balance": 2465, "rateLimit": 5}))]).await;
        assert_eq!(Client::with(&base, "sk").balance().await.unwrap(), 2465);
        assert_eq!(seen.auth.lock().unwrap()[0], "sk");
    }

    #[tokio::test]
    async fn failures_are_told_apart_by_status_and_reason() {
        let kind = |status: u16, headers: Vec<(&'static str, &'static str)>, body: Value| async move {
            let (base, _) = serve(vec![("/search", status, headers, body)]).await;
            let e = Client::with(&base, "k").search_hits("x", 5, SearchOpts::default()).await.unwrap_err();
            let pe = e.downcast::<ProviderError>().unwrap();
            (pe.kind, pe.retry_after, pe.message)
        };
        let (k, _, msg) = kind(403, vec![], json!({"message": "Unauthorized.", "statusCode": 403})).await;
        assert_eq!(k, Kind::BadKey);
        assert!(msg.contains("403") && msg.contains("Unauthorized."), "{msg}");
        assert_eq!(kind(400, vec![], json!({"message": "Not enough credits", "statusCode": 400})).await.0, Kind::Exhausted);
        assert_eq!(kind(400, vec![], json!({"message": "Missing query parameter", "statusCode": 400})).await.0, Kind::Rejected);
        assert_eq!(kind(402, vec![], json!({"message": "Payment required"})).await.0, Kind::Exhausted);
        let (k, wait, _) = kind(429, vec![("retry-after", "3")], json!({"message": "Too many requests"})).await;
        assert_eq!((k, wait), (Kind::RateLimited, Some(Duration::from_secs(3))));
        assert_eq!(kind(503, vec![], json!({})).await.0, Kind::Server);
    }

    /// Against the real service, with `SERPER_API_KEY` set: `cargo test live_serper -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs SERPER_API_KEY and spends a few credits"]
    async fn live_serper_searches_in_the_shared_shapes() {
        let c = Client::with(DEFAULT_URL, &std::env::var("SERPER_API_KEY").expect("SERPER_API_KEY"));
        let before = c.balance().await.unwrap();
        let s = c.search_hits("rust programming language", 3, SearchOpts::default()).await.unwrap();
        println!("search: {} hits, {} credits, first {:?}", s.value.len(), s.credits, s.value.first());
        assert!(s.value.len() == 3 && s.value.iter().all(|h| h.url.starts_with("http") && !h.title.is_empty()));
        let n = c.search_hits("rust release", 4, SearchOpts { recency: Some(Recency::Week), news: true }).await.unwrap();
        println!("news: {} hits, {} credits, first {:?}", n.value.len(), n.credits, n.value.first());
        assert!(n.value.len() == 4 && n.value[0].published.is_some());
        println!("balance: {before} -> {}", c.balance().await.unwrap());
        let bad = Client::with(DEFAULT_URL, "not-a-key").search_hits("x", 1, SearchOpts::default()).await.unwrap_err();
        assert_eq!(bad.downcast::<ProviderError>().unwrap().kind, Kind::BadKey);
    }
}
