//! The web tools an agent is given: what they are called, what they take, and running one. Shared by the MCP endpoint
//! (agent CLIs) and the direct-call loop (models with no CLI), so both offer the same thing. Every call goes through the
//! pool, so it is served by whichever web account can.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::client::MAX_CRAWL_PAGES;
use super::pool::{Budget, Pool};
use super::render;
use super::types::{Call, Output, Recency, SearchOpts, Tool};

pub const SEARCH: &str = "web_search";
pub const SCRAPE: &str = "scrape";
pub const MAP: &str = "map";
pub const CRAWL: &str = "crawl";
/// Pages crawled for a documentation snapshot.
pub const DOCS_PAGES: u32 = 10;

/// The tools the pool can serve, in the shape MCP lists them. `annotations` tell a CLI they only read, so it does not
/// stop to ask. A tool no account can do is left out, so an agent is never offered what would fail.
pub fn definitions(pool: &Pool) -> Vec<Value> {
    let ro = json!({"readOnlyHint": true, "openWorldHint": true, "destructiveHint": false, "idempotentHint": true});
    let all = [
        (Tool::Search, json!({"name": SEARCH, "annotations": ro, "description": "Search the web. Returns a numbered list of results with title, address and a short description. Uses credits on a paid account; ask for as few results as you need.",
               "inputSchema": {"type": "object", "properties": {
                   "query": {"type": "string", "description": "What to search for."},
                   "limit": {"type": "integer", "minimum": 1, "maximum": 10, "description": "How many results, default 5."},
                   "recency": {"type": "string", "enum": ["day", "week", "month", "year"], "description": "Only results from this long ago or newer."},
                   "news": {"type": "boolean", "description": "News stories rather than general pages."}}, "required": ["query"]}})),
        (Tool::Scrape, json!({"name": SCRAPE, "annotations": ro, "description": "Fetch one web page and return its main text as markdown. Handles pages that need JavaScript. Uses about 1 credit.",
               "inputSchema": {"type": "object", "properties": {"url": {"type": "string", "description": "The page address, starting with http:// or https://."}}, "required": ["url"]}})),
        (Tool::Map, json!({"name": MAP, "annotations": ro, "description": "List the pages of a website, to find the one you want before scraping it. Uses about 1 credit.",
               "inputSchema": {"type": "object", "properties": {"url": {"type": "string", "description": "The site's address."}, "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Most addresses to return, default 50."}}, "required": ["url"]}})),
        (Tool::Crawl, json!({"name": CRAWL, "annotations": ro, "description": format!("Fetch several pages of a site, starting from one address, and return each page's main text. Uses about 1 credit per page, so keep the limit small (at most {MAX_CRAWL_PAGES}). Slow: prefer scrape when you know the page."),
               "inputSchema": {"type": "object", "properties": {"url": {"type": "string", "description": "Where to start."}, "limit": {"type": "integer", "minimum": 1, "maximum": MAX_CRAWL_PAGES, "description": "Most pages to fetch, default 5."}}, "required": ["url"]}})),
    ];
    all.into_iter().filter(|(t, _)| pool.offers(*t)).map(|(_, d)| d).collect()
}

/// Whether `url` is somewhere a web tool should fetch: http or https, and not this machine or its own network. A
/// hosted Firecrawl could not reach those anyway, but one run at home could.
pub fn url_ok(url: &str) -> Result<(), String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).ok_or("the address must start with http:// or https://")?;
    if url.len() > 2000 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("the address is not valid".into());
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or("");
    let host = if host.starts_with('[') { host.split(']').next().unwrap_or("").trim_start_matches('[') } else { host.split(':').next().unwrap_or("") }.to_lowercase();
    let private = host.is_empty()
        || host == "localhost"
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host == "::1"
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("169.254.")
        || host.starts_with("0.")
        || host.strip_prefix("172.").and_then(|r| r.split('.').next()).and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (16..=31).contains(&n));
    if private { Err("that address is on this machine or its own network".into()) } else { Ok(()) }
}

/// Most addresses taken from one piece of text.
pub const MAX_LINKS: usize = 3;

/// The web addresses in `text`, in order, each once: what a person pasted into a goal. Punctuation that ends a sentence is
/// not part of the address, and addresses on this machine or its network are left out.
pub fn links_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || "<>\"'`".contains(c)) {
        let Some(at) = word.find("http://").or_else(|| word.find("https://")) else { continue };
        let mut url = &word[at..];
        // A link in brackets or at the end of a sentence: take the trailing marks off, but keep a closing bracket that has its opener inside.
        while let Some(last) = url.chars().last() {
            let open = match last {
                ')' => Some('('),
                ']' => Some('['),
                _ => None,
            };
            if ".,;:!?".contains(last) || open.is_some_and(|o| url.matches(o).count() < url.matches(last).count()) {
                url = &url[..url.len() - last.len_utf8()];
            } else {
                break;
            }
        }
        if url_ok(url).is_ok() && !out.iter().any(|u| u == url) {
            out.push(url.to_owned());
            if out.len() == MAX_LINKS {
                break;
            }
        }
    }
    out
}

fn text_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    match args.get(key).and_then(Value::as_str).map(str::trim) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => bail!("`{key}` is required"),
    }
}

fn int_arg(args: &Value, key: &str, default: u32) -> u32 {
    args.get(key).and_then(Value::as_u64).map_or(default, |n| n.min(u32::MAX as u64) as u32)
}

/// The tools as a direct-call model is offered them.
pub fn chat_tools(pool: &Pool) -> Vec<crate::providers::chat::ToolDef> {
    definitions(pool)
        .into_iter()
        .filter_map(|d| {
            Some(crate::providers::chat::ToolDef {
                name: d.get("name")?.as_str()?.to_owned(),
                description: d.get("description")?.as_str()?.to_owned(),
                parameters: d.get("inputSchema")?.clone(),
            })
        })
        .collect()
}

/// Lets a direct-call model use the web tools through the pool. Failures come back as text the model can read.
pub struct Web(pub Pool);

impl crate::providers::chat::ToolRunner for Web {
    async fn call(&self, name: &str, args: &Value) -> (String, u64, f64) {
        match call(&self.0, None, name, args).await {
            Ok(out) => (out.text, out.credits, out.usd),
            Err(e) => (format!("{e:#}"), 0, 0.0),
        }
    }
}

/// What an agent reads: the answer, and a closing line saying who gave it and who was passed over.
fn answer<T>(body: String, c: &Call<T>) -> Output {
    Output { text: format!("{body}\n\n{}", render::provenance(c.source, &c.notes)), credits: c.credits, usd: c.usd }
}

/// Run one tool.
pub async fn call(pool: &Pool, budget: Option<&Budget<'_>>, name: &str, args: &Value) -> Result<Output> {
    match name {
        SEARCH => {
            let opts = SearchOpts { recency: args.get("recency").and_then(Value::as_str).and_then(Recency::parse), news: args.get("news").and_then(Value::as_bool).unwrap_or(false) };
            let c = pool.search(text_arg(args, "query")?, int_arg(args, "limit", 5), opts, budget).await?;
            Ok(answer(render::search(&c.value), &c))
        }
        SCRAPE | MAP | CRAWL => {
            let url = text_arg(args, "url")?;
            url_ok(url).map_err(anyhow::Error::msg)?;
            match name {
                SCRAPE => {
                    let c = pool.scrape(url, budget).await?;
                    Ok(answer(render::page(&c.value, render::PAGE), &c))
                }
                MAP => {
                    let c = pool.map(url, int_arg(args, "limit", 50), budget).await?;
                    Ok(answer(render::map(&c.value), &c))
                }
                _ => {
                    let c = pool.crawl(url, int_arg(args, "limit", 5), budget).await?;
                    Ok(answer(render::crawl(&c.value, render::CRAWLED_PAGE, render::ANSWER), &c))
                }
            }
        }
        other => bail!("there is no tool called {other}"),
    }
}

/// One page's text for reference material, with no note about who served it.
pub async fn page_text(pool: &Pool, url: &str) -> Result<Output> {
    let c = pool.scrape(url, None).await?;
    Ok(Output { text: render::page(&c.value, render::PAGE), credits: c.credits, usd: c.usd })
}

/// A documentation site read in full, to be kept in files rather than handed straight to a model.
pub async fn docs_text(pool: &Pool, url: &str) -> Result<Output> {
    let c = pool.crawl(url, DOCS_PAGES, None).await?;
    Ok(Output { text: render::crawl(&c.value, render::DOC_PAGE, render::DOC_TOTAL), credits: c.credits, usd: c.usd })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::web::client::Client;
    use crate::web::pool::{Cooldowns, Member};
    use crate::web::serper;
    use crate::web::testutil::{ok, serve};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn pool(members: Vec<Member>) -> Pool {
        Pool::new(members, HashMap::new(), Arc::new(Cooldowns::default()))
    }

    fn firecrawl(base: &str) -> Member {
        Member::firecrawl("fc", 0.01, Client::with(base, "k"))
    }

    #[test]
    fn every_tool_is_described_with_a_schema_and_marked_read_only() {
        let defs = definitions(&pool(vec![firecrawl("http://x.test")]));
        assert_eq!(defs.iter().map(|d| d["name"].as_str().unwrap()).collect::<Vec<_>>(), ["web_search", "scrape", "map", "crawl"]);
        for d in &defs {
            assert_eq!(d["inputSchema"]["type"], "object");
            assert!(d["inputSchema"]["required"].as_array().is_some_and(|r| r.len() == 1));
            assert_eq!(d["annotations"]["readOnlyHint"], true, "so a CLI does not stop to ask");
            assert!(d["description"].as_str().unwrap().contains("credit"), "the agent is told what it costs");
        }
        assert_eq!(defs[3]["inputSchema"]["properties"]["limit"]["maximum"], MAX_CRAWL_PAGES);
        assert_eq!(defs[0]["inputSchema"]["properties"]["recency"]["enum"], json!(["day", "week", "month", "year"]));
    }

    #[test]
    fn only_the_tools_some_account_can_do_are_offered() {
        let names = |p: &Pool| definitions(p).iter().map(|d| d["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        let serper = Member::serper("sv", 0.001, serper::Client::with("http://y.test", "k"));
        assert_eq!(names(&pool(vec![serper])), ["web_search"], "Serper only searches");
        assert!(names(&pool(vec![])).is_empty());
        assert!(chat_tools(&pool(vec![])).is_empty());
    }

    #[test]
    fn a_direct_model_is_offered_the_same_four_tools() {
        let t = chat_tools(&pool(vec![firecrawl("http://x.test")]));
        assert_eq!(t.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["web_search", "scrape", "map", "crawl"]);
        assert!(t.iter().all(|t| t.parameters["type"] == "object" && !t.description.is_empty()));
    }

    #[test]
    fn addresses_on_this_machine_or_its_network_are_refused() {
        for ok in ["https://example.com", "http://docs.rs/x?y=1#z", "https://sub.example.co.uk:8443/p", "https://172.32.0.1/", "https://8.8.8.8/"] {
            assert_eq!(url_ok(ok), Ok(()), "{ok}");
        }
        for bad in [
            "ftp://example.com", "example.com", "file:///etc/passwd", "http://localhost:3000", "http://127.0.0.1/", "https://10.1.2.3/", "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data", "http://172.16.0.1/", "http://172.31.255.255/", "http://[::1]:80/", "http://printer.local/", "http://db.internal/",
            "http://user:pw@localhost/", "https://", "https://exa mple.com",
        ] {
            assert!(url_ok(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn links_are_found_in_a_goal_without_the_punctuation_around_them() {
        let goal = "Build it like https://example.com/docs, and see (https://docs.rs/serde/latest/serde/). Also <https://a.test/x?y=1>. Not http://localhost:3000 or ftp://x.test or https://example.com/docs again.";
        assert_eq!(links_in(goal), ["https://example.com/docs", "https://docs.rs/serde/latest/serde/", "https://a.test/x?y=1"]);
        assert_eq!(links_in("a bracket inside stays: https://en.wikipedia.org/wiki/Rust_(programming_language)."), ["https://en.wikipedia.org/wiki/Rust_(programming_language)"]);
        assert!(links_in("no links, only the word http and example.com").is_empty());
        let many: String = (0..9).map(|i| format!("https://s{i}.test/ ")).collect();
        assert_eq!(links_in(&many).len(), MAX_LINKS, "never more than a few");
        assert!(links_in("http://192.168.0.1/admin https://10.0.0.5/").is_empty(), "this machine's network is not fetched for a stranger's text");
    }

    #[tokio::test]
    async fn bad_arguments_and_unknown_tools_are_errors_the_agent_can_read() {
        let p = pool(vec![firecrawl("http://127.0.0.1:1")]);
        let err = |name: &str, args: Value| {
            let p = &p;
            let name = name.to_owned();
            async move { call(p, None, &name, &args).await.unwrap_err().to_string() }
        };
        assert!(err(SEARCH, json!({})).await.contains("`query` is required"));
        assert!(err(SCRAPE, json!({"url": "  "})).await.contains("`url` is required"));
        assert!(err(SCRAPE, json!({"url": "http://localhost/x"})).await.contains("this machine"));
        assert!(err("delete_everything", json!({})).await.contains("no tool called"));
    }

    #[tokio::test]
    async fn an_answer_says_who_gave_it_and_search_options_reach_the_account() {
        let (base, seen) = serve(vec![ok("/v2/search", json!({"creditsUsed": 2, "data": {"news": [{"url": "https://n.test", "title": "N", "snippet": "s", "date": "1 day ago"}]}}))]).await;
        let p = pool(vec![firecrawl(&base)]);
        let out = call(&p, None, SEARCH, &json!({"query": "rust", "recency": "week", "news": true, "limit": 3})).await.unwrap();
        assert_eq!(out.text, "1. N — https://n.test (1 day ago)\n   s\n\n(via Firecrawl)");
        assert_eq!((out.credits, out.usd), (2, 0.02));
        let b = seen.bodies.lock().unwrap()[0].1.clone();
        assert_eq!((b["tbs"].clone(), b["sources"].clone(), b["limit"].clone()), (json!("qdr:w"), json!(["news"]), json!(3)));
    }

    #[tokio::test]
    async fn when_no_account_has_anything_the_answer_says_so_and_names_who_was_asked() {
        let (base, _) = serve(vec![ok("/v2/search", json!({"creditsUsed": 1, "data": {"web": []}}))]).await;
        let out = call(&pool(vec![firecrawl(&base)]), None, SEARCH, &json!({"query": "zzz"})).await.unwrap();
        assert_eq!(out.text, "No results.\n\n(no web account had an answer: Firecrawl found nothing)");
        assert_eq!(out.credits, 1, "and what the empty search cost is still counted");
    }
}
