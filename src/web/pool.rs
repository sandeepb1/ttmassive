//! The web accounts, used as one. A call goes to the first account that can do it, in an order set per tool, and moves
//! to the next when an account fails or has nothing. Accounts that are out of credits or refuse their key are left
//! alone for a while.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use super::client::Client;
use super::error::Kind;
use super::serper;
use super::tavily;
use super::error::ProviderError;
use super::types::{Call, Crawl, Fetched, Page, Provider, SearchHit, SearchOpts, SiteMap, Tool};

enum Backend {
    Firecrawl(Client),
    Tavily(tavily::Client),
    Serper(serper::Client),
}

/// One account in the pool.
pub struct Member {
    /// The account's id, which the order refers to.
    pub id: String,
    /// What one of the service's credits is worth, to put a price on a call.
    pub usd_per_credit: f64,
    backend: Backend,
}

impl Member {
    pub fn firecrawl(id: &str, usd_per_credit: f64, client: Client) -> Member {
        Member { id: id.into(), usd_per_credit, backend: Backend::Firecrawl(client) }
    }

    pub fn tavily(id: &str, usd_per_credit: f64, client: tavily::Client) -> Member {
        Member { id: id.into(), usd_per_credit, backend: Backend::Tavily(client) }
    }

    pub fn serper(id: &str, usd_per_credit: f64, client: serper::Client) -> Member {
        Member { id: id.into(), usd_per_credit, backend: Backend::Serper(client) }
    }

    pub fn provider(&self) -> Provider {
        match self.backend {
            Backend::Firecrawl(_) => Provider::Firecrawl,
            Backend::Tavily(_) => Provider::Tavily,
            Backend::Serper(_) => Provider::Serper,
        }
    }

    async fn search(&self, query: &str, limit: u32, opts: SearchOpts) -> Result<Fetched<Vec<SearchHit>>> {
        match &self.backend {
            Backend::Firecrawl(c) => c.search_hits(query, limit, opts).await,
            Backend::Tavily(c) => c.search_hits(query, limit, opts).await,
            Backend::Serper(c) => c.search_hits(query, limit, opts).await,
        }
    }

    async fn scrape(&self, url: &str) -> Result<Fetched<Page>> {
        match &self.backend {
            Backend::Firecrawl(c) => c.scrape_page(url).await,
            Backend::Tavily(c) => c.scrape_page(url).await,
            Backend::Serper(_) => Err(only_searches()),
        }
    }

    async fn map(&self, url: &str, limit: u32) -> Result<Fetched<SiteMap>> {
        match &self.backend {
            Backend::Firecrawl(c) => c.map_site(url, limit).await,
            Backend::Tavily(c) => c.map_site(url, limit).await,
            Backend::Serper(_) => Err(only_searches()),
        }
    }

    async fn crawl(&self, url: &str, limit: u32) -> Result<Fetched<Crawl>> {
        match &self.backend {
            Backend::Firecrawl(c) => c.crawl_site(url, limit).await,
            Backend::Tavily(c) => c.crawl_site(url, limit).await,
            Backend::Serper(_) => Err(only_searches()),
        }
    }
}

fn only_searches() -> anyhow::Error {
    ProviderError::new(Kind::Unsupported, "Serper only searches").into()
}

/// Accounts that are being left alone, and why. Shared, so a rest outlasts the pool built for one call.
#[derive(Default)]
pub struct Cooldowns(Mutex<HashMap<String, (Instant, &'static str)>>);

impl Cooldowns {
    pub fn rest(&self, account: &str, for_: Duration, why: &'static str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(account.to_owned(), (Instant::now() + for_, why));
    }

    /// Why `account` is resting at `now`, if it is.
    pub fn resting_at(&self, account: &str, now: Instant) -> Option<&'static str> {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(account) {
            Some(&(until, why)) if until > now => Some(why),
            Some(_) => {
                map.remove(account);
                None
            }
            None => None,
        }
    }

    fn resting(&self, account: &str) -> Option<&'static str> {
        self.resting_at(account, Instant::now())
    }
}

/// How a run limits what the pool may spend.
pub struct Budget<'a> {
    /// Asked before each attempt: whether the run may still spend.
    pub allowed: &'a dyn Fn() -> bool,
    /// Told what each attempt cost, whether or not it found anything.
    pub record: &'a dyn Fn(u64, f64),
}

pub struct Pool {
    members: Vec<Member>,
    /// Account ids, first choice first, where the order was set by hand.
    order: HashMap<Tool, Vec<String>>,
    cooldowns: Arc<Cooldowns>,
}

impl Pool {
    pub fn new(members: Vec<Member>, order: HashMap<Tool, Vec<String>>, cooldowns: Arc<Cooldowns>) -> Pool {
        Pool { members, order, cooldowns }
    }

    /// Whether any account can do `tool`.
    pub fn offers(&self, tool: Tool) -> bool {
        self.members.iter().any(|m| m.provider().can(tool))
    }

    /// The accounts that can do `tool`, in the order to ask them.
    fn candidates(&self, tool: Tool) -> Vec<&Member> {
        let by_hand = self.order.get(&tool);
        let mut found: Vec<(usize, usize, usize, &Member)> = self
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| m.provider().can(tool))
            .map(|(i, m)| {
                let listed = by_hand.and_then(|o| o.iter().position(|id| *id == m.id)).unwrap_or(usize::MAX);
                let rank = tool.default_order().iter().position(|p| *p == m.provider()).unwrap_or(usize::MAX);
                (listed, rank, i, m)
            })
            .collect();
        found.sort_by_key(|&(listed, rank, i, _)| (listed, rank, i));
        found.into_iter().map(|(.., m)| m).collect()
    }

    pub async fn search(&self, query: &str, limit: u32, opts: SearchOpts, budget: Option<&Budget<'_>>) -> Result<Call<Vec<SearchHit>>> {
        self.run(Tool::Search, budget, async |m: &Member| m.search(query, limit, opts).await, |v: &Vec<SearchHit>| v.is_empty()).await
    }

    pub async fn scrape(&self, url: &str, budget: Option<&Budget<'_>>) -> Result<Call<Page>> {
        self.run(Tool::Scrape, budget, async |m: &Member| m.scrape(url).await, |p: &Page| p.markdown.is_empty()).await
    }

    pub async fn map(&self, url: &str, limit: u32, budget: Option<&Budget<'_>>) -> Result<Call<SiteMap>> {
        self.run(Tool::Map, budget, async |m: &Member| m.map(url, limit).await, |s: &SiteMap| s.pages.is_empty()).await
    }

    pub async fn crawl(&self, url: &str, limit: u32, budget: Option<&Budget<'_>>) -> Result<Call<Crawl>> {
        self.run(Tool::Crawl, budget, async |m: &Member| m.crawl(url, limit).await, |c: &Crawl| c.pages.is_empty()).await
    }

    /// Ask each capable account once, in order, until one has something. What every attempt cost is counted, found or
    /// not. If all had nothing the answer is empty and says who was asked; if all failed it is an error saying why.
    async fn run<T>(&self, tool: Tool, budget: Option<&Budget<'_>>, op: impl AsyncFn(&Member) -> Result<Fetched<T>>, is_empty: impl Fn(&T) -> bool) -> Result<Call<T>> {
        let (mut notes, mut credits, mut usd) = (Vec::new(), 0u64, 0.0f64);
        let mut empty = None;
        for m in self.candidates(tool) {
            let name = m.provider().name();
            if let Some(why) = self.cooldowns.resting(&m.id) {
                notes.push(format!("{name} is resting ({why})"));
                continue;
            }
            if budget.is_some_and(|b| !(b.allowed)()) {
                notes.push("the web cost cap for this run is reached".into());
                break;
            }
            match op(m).await {
                Ok(f) => {
                    let cost = f.credits as f64 * m.usd_per_credit;
                    if let Some(b) = budget {
                        (b.record)(f.credits, cost);
                    }
                    credits += f.credits;
                    usd += cost;
                    if is_empty(&f.value) {
                        notes.push(format!("{name} found nothing"));
                        empty = Some(f.value);
                    } else {
                        return Ok(Call { value: f.value, credits, usd, source: Some(m.provider()), notes });
                    }
                }
                Err(e) => match e.downcast_ref::<ProviderError>() {
                    Some(pe) if !pe.kind.fails_over() => return Err(e),
                    Some(pe) => {
                        if let Some(d) = pe.rest() {
                            self.cooldowns.rest(&m.id, d, pe.kind.label());
                        }
                        notes.push(format!("{name} {}", pe.kind.label()));
                    }
                    None => notes.push(format!("{name} failed: {e:#}")),
                },
            }
        }
        match empty {
            Some(value) => Ok(Call { value, credits, usd, source: None, notes }),
            None if notes.is_empty() => bail!("No web account is set up to {}.", tool.verb()),
            None => bail!("No web account could {}: {}.", tool.verb(), notes.join("; ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, http::StatusCode, routing::post};
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const FOUND: &str = r#"{"creditsUsed": 2, "data": {"web": [{"url": "https://a.test", "title": "A", "description": "d"}]}}"#;
    const NONE: &str = r#"{"creditsUsed": 1, "data": {"web": []}}"#;

    /// A stand-in Firecrawl that always answers with `status` and `body`, counting the calls it gets.
    async fn mock(status: u16, body: &'static str) -> (String, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let answer = move || {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                (StatusCode::from_u16(status).unwrap(), [("content-type", "application/json")], body)
            }
        };
        let app = Router::new().route("/v2/search", post(answer.clone())).route("/v2/scrape", post(answer));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, hits)
    }

    fn member(id: &str, base: &str, rate: f64) -> Member {
        Member::firecrawl(id, rate, Client::with(base, "key"))
    }

    fn pool(members: Vec<Member>) -> (Pool, Arc<Cooldowns>) {
        let cool = Arc::new(Cooldowns::default());
        (Pool::new(members, HashMap::new(), cool.clone()), cool)
    }

    #[tokio::test]
    async fn the_first_account_that_has_an_answer_serves_it() {
        let (a, a_hits) = mock(200, FOUND).await;
        let (b, b_hits) = mock(200, FOUND).await;
        let (p, _) = pool(vec![member("a", &a, 0.01), member("b", &b, 0.5)]);
        let call = p.search("rust", 5, SearchOpts::default(), None).await.unwrap();
        assert_eq!((call.source, call.credits, call.notes.len()), (Some(Provider::Firecrawl), 2, 0));
        assert!((call.usd - 0.02).abs() < 1e-9, "priced at the serving account's rate: {}", call.usd);
        assert_eq!((a_hits.load(Ordering::SeqCst), b_hits.load(Ordering::SeqCst)), (1, 0));
    }

    #[tokio::test]
    async fn an_account_without_credits_is_passed_over_and_then_left_alone() {
        let (a, a_hits) = mock(402, r#"{"error": "Insufficient credits"}"#).await;
        let (b, b_hits) = mock(200, FOUND).await;
        let (p, cool) = pool(vec![member("a", &a, 0.01), member("b", &b, 0.01)]);
        let call = p.search("rust", 5, SearchOpts::default(), None).await.unwrap();
        assert_eq!(call.value[0].title, "A");
        assert_eq!(call.notes, vec!["Firecrawl is out of credits".to_string()]);
        assert_eq!(cool.resting("a"), Some("is out of credits"));
        p.search("rust", 5, SearchOpts::default(), None).await.unwrap();
        assert_eq!((a_hits.load(Ordering::SeqCst), b_hits.load(Ordering::SeqCst)), (1, 2), "the second call did not ask a again");
    }

    #[tokio::test]
    async fn a_server_error_is_passed_over_without_a_rest() {
        let (a, a_hits) = mock(503, "{}").await;
        let (b, _) = mock(200, FOUND).await;
        let (p, cool) = pool(vec![member("a", &a, 0.01), member("b", &b, 0.01)]);
        p.search("x", 5, SearchOpts::default(), None).await.unwrap();
        p.search("x", 5, SearchOpts::default(), None).await.unwrap();
        assert_eq!(a_hits.load(Ordering::SeqCst), 2);
        assert_eq!(cool.resting("a"), None);
    }

    #[tokio::test]
    async fn a_rate_limited_account_rests_for_a_while() {
        let (a, _) = mock(429, r#"{"error": "slow down"}"#).await;
        let (b, _) = mock(200, FOUND).await;
        let (p, cool) = pool(vec![member("a", &a, 0.01), member("b", &b, 0.01)]);
        p.search("x", 5, SearchOpts::default(), None).await.unwrap();
        assert_eq!(cool.resting("a"), Some("is rate limited"));
        let later = Instant::now() + Duration::from_secs(61);
        assert_eq!(cool.resting_at("a", later), None, "and then is asked again");
    }

    #[tokio::test]
    async fn an_empty_answer_moves_on_and_what_it_cost_is_counted() {
        let (a, _) = mock(200, NONE).await;
        let (b, _) = mock(200, FOUND).await;
        let (p, _) = pool(vec![member("a", &a, 0.01), member("b", &b, 0.1)]);
        let spent = Mutex::new((0u64, 0.0f64));
        let allowed = || true;
        let record = |c: u64, u: f64| {
            let mut s = spent.lock().unwrap();
            s.0 += c;
            s.1 += u;
        };
        let call = p.search("x", 5, SearchOpts::default(), Some(&Budget { allowed: &allowed, record: &record })).await.unwrap();
        assert_eq!(call.notes, vec!["Firecrawl found nothing".to_string()]);
        assert_eq!(call.credits, 3, "the empty attempt's credit and the answer's two");
        let (c, u) = *spent.lock().unwrap();
        assert_eq!(c, 3);
        assert!((u - (0.01 + 0.2)).abs() < 1e-9 && (call.usd - u).abs() < 1e-9, "{u} {}", call.usd);
    }

    #[tokio::test]
    async fn when_every_account_has_nothing_the_answer_is_empty_and_says_so() {
        let (a, _) = mock(200, NONE).await;
        let (b, _) = mock(200, NONE).await;
        let (p, _) = pool(vec![member("a", &a, 0.01), member("b", &b, 0.01)]);
        let call = p.search("x", 5, SearchOpts::default(), None).await.unwrap();
        assert!(call.value.is_empty() && call.source.is_none());
        assert_eq!(call.notes.len(), 2);
        assert_eq!(call.credits, 2);
    }

    #[tokio::test]
    async fn when_every_account_fails_the_error_says_why() {
        let (a, _) = mock(401, r#"{"error": "bad key"}"#).await;
        let (b, _) = mock(503, "{}").await;
        let (p, _) = pool(vec![member("a", &a, 0.01), member("b", &b, 0.01)]);
        let err = p.search("x", 5, SearchOpts::default(), None).await.unwrap_err().to_string();
        assert!(err.starts_with("No web account could search:") && err.contains("refused its key") && err.contains("had a server error"), "{err}");
        assert_eq!(Pool::new(vec![], HashMap::new(), Arc::default()).search("x", 5, SearchOpts::default(), None).await.unwrap_err().to_string(), "No web account is set up to search.");
    }

    #[tokio::test]
    async fn a_refused_request_is_not_taken_to_another_account() {
        let (a, _) = mock(400, r#"{"error": "bad url"}"#).await;
        let (b, b_hits) = mock(200, FOUND).await;
        let (p, _) = pool(vec![member("a", &a, 0.01), member("b", &b, 0.01)]);
        let err = p.scrape("https://x.test", None).await.unwrap_err().to_string();
        assert!(err.contains("400") && err.contains("bad url"), "{err}");
        assert_eq!(b_hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_cost_cap_stops_the_search_between_accounts() {
        let (a, _) = mock(200, NONE).await;
        let (b, b_hits) = mock(200, FOUND).await;
        let (p, _) = pool(vec![member("a", &a, 0.01), member("b", &b, 0.01)]);
        let used = AtomicUsize::new(0);
        let allowed = || used.load(Ordering::SeqCst) == 0;
        let record = |c: u64, _: f64| {
            used.fetch_add(c as usize, Ordering::SeqCst);
        };
        let call = p.search("x", 5, SearchOpts::default(), Some(&Budget { allowed: &allowed, record: &record })).await.unwrap();
        assert!(call.value.is_empty() && call.notes.iter().any(|n| n.contains("cost cap")), "{:?}", call.notes);
        assert_eq!(b_hits.load(Ordering::SeqCst), 0);
        let none_left = || false;
        let err = p.search("x", 5, SearchOpts::default(), Some(&Budget { allowed: &none_left, record: &record })).await.unwrap_err().to_string();
        assert!(err.contains("cost cap"), "{err}");
    }

    #[tokio::test]
    async fn the_order_set_by_hand_beats_the_order_accounts_were_added_in() {
        let (a, a_hits) = mock(200, FOUND).await;
        let (b, b_hits) = mock(200, FOUND).await;
        let cool = Arc::new(Cooldowns::default());
        let order = HashMap::from([(Tool::Search, vec!["b".to_string()])]);
        let p = Pool::new(vec![member("a", &a, 0.01), member("b", &b, 0.01)], order, cool);
        p.search("x", 5, SearchOpts::default(), None).await.unwrap();
        assert_eq!((a_hits.load(Ordering::SeqCst), b_hits.load(Ordering::SeqCst)), (0, 1));
        assert_eq!(p.candidates(Tool::Scrape).iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b"], "other tools keep the default");
    }

    #[tokio::test]
    async fn tavily_takes_over_when_firecrawl_has_no_credits() {
        let (fc, _) = mock(402, r#"{"error": "Insufficient credits"}"#).await;
        let app = Router::new()
            .route("/search", post(|| async { axum::Json(serde_json::json!({"usage": {"credits": 1}, "results": [{"title": "T", "url": "https://t.test", "content": "c", "score": 0.7}]})) }))
            .route("/extract", post(|| async { axum::Json(serde_json::json!({"usage": {"credits": 1}, "results": [{"url": "https://x.test", "raw_content": "from tavily"}]})) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tv = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (p, _) = pool(vec![member("fc", &fc, 0.01), Member::tavily("tv", 0.008, tavily::Client::with(&tv, "tvly-k"))]);
        // Tavily is first for search by default, so it answers outright.
        let call = p.search("x", 5, SearchOpts::default(), None).await.unwrap();
        assert_eq!((call.source, call.credits, call.value[0].score), (Some(Provider::Tavily), 1, Some(0.7)));
        assert!((call.usd - 0.008).abs() < 1e-9);
        // Scrape is Firecrawl's first, then Tavily's: with Firecrawl out of credits Tavily is asked.
        let page = p.scrape("https://x.test", None).await.unwrap();
        assert_eq!((page.source, page.value.markdown.as_str(), page.notes), (Some(Provider::Tavily), "from tavily", vec!["Firecrawl is out of credits".to_string()]));
    }

    #[test]
    fn serper_is_asked_to_search_first_and_never_to_fetch() {
        let (p, _) = pool(vec![
            member("fc", "http://x.test", 0.01),
            Member::tavily("tv", 0.008, tavily::Client::with("http://y.test", "k")),
            Member::serper("sv", 0.001, serper::Client::with("http://z.test", "k")),
        ]);
        let ids = |t| p.candidates(t).iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(Tool::Search), ["sv", "tv", "fc"]);
        assert_eq!(ids(Tool::Scrape), ["fc", "tv"]);
        assert_eq!(ids(Tool::Map), ["fc", "tv"]);
        let (only, _) = pool(vec![Member::serper("sv", 0.001, serper::Client::with("http://z.test", "k"))]);
        assert!(only.offers(Tool::Search) && !only.offers(Tool::Scrape) && !only.offers(Tool::Map) && !only.offers(Tool::Crawl));
    }

    #[tokio::test]
    async fn a_search_falls_from_serper_to_the_next_account_when_serper_has_no_credits() {
        use crate::web::testutil::serve;
        let (sv, _) = serve(vec![("/search", 400, vec![], serde_json::json!({"message": "Not enough credits"}))]).await;
        let (fc, _) = mock(200, FOUND).await;
        let (p, cool) = pool(vec![member("fc", &fc, 0.01), Member::serper("sv", 0.001, serper::Client::with(&sv, "k"))]);
        let call = p.search("x", 5, SearchOpts::default(), None).await.unwrap();
        assert_eq!((call.source, call.notes), (Some(Provider::Firecrawl), vec!["Serper is out of credits".to_string()]));
        assert_eq!(cool.resting("sv"), Some("is out of credits"));
    }

    #[test]
    fn each_tool_has_its_own_default_order() {
        let (p, _) = pool(vec![member("fc", "http://x.test", 0.01), Member::tavily("tv", 0.008, tavily::Client::with("http://y.test", "k"))]);
        let ids = |t| p.candidates(t).iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(Tool::Search), ["tv", "fc"]);
        assert_eq!(ids(Tool::Scrape), ["fc", "tv"]);
        assert_eq!(ids(Tool::Crawl), ["fc", "tv"]);
    }

    #[test]
    fn a_tool_is_offered_only_when_an_account_can_do_it() {
        let (p, _) = pool(vec![]);
        assert!(!p.offers(Tool::Search));
        let (p, _) = pool(vec![member("a", "http://x.test", 0.01)]);
        assert!(p.offers(Tool::Search) && p.offers(Tool::Crawl));
        let _: Value = json!({});
    }
}
