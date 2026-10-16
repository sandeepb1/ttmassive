//! The web tools an agent is given: what they are called, what they take, and running one. Shared by the MCP endpoint
//! (agent CLIs) and the direct-call loop (models with no CLI), so both offer the same thing.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::client::{Client, MAX_CRAWL_PAGES, Output};

pub const SEARCH: &str = "web_search";
pub const SCRAPE: &str = "scrape";
pub const MAP: &str = "map";
pub const CRAWL: &str = "crawl";

/// The tools, in the shape MCP lists them. `annotations` tell a CLI they only read, so it does not stop to ask.
pub fn definitions() -> Vec<Value> {
    let ro = json!({"readOnlyHint": true, "openWorldHint": true, "destructiveHint": false, "idempotentHint": true});
    vec![
        json!({"name": SEARCH, "annotations": ro, "description": "Search the web. Returns a numbered list of results with title, address and a short description. Uses about 2 credits per 10 results; ask for as few as you need.",
               "inputSchema": {"type": "object", "properties": {"query": {"type": "string", "description": "What to search for."}, "limit": {"type": "integer", "minimum": 1, "maximum": 10, "description": "How many results, default 5."}}, "required": ["query"]}}),
        json!({"name": SCRAPE, "annotations": ro, "description": "Fetch one web page and return its main text as markdown. Handles pages that need JavaScript. Uses 1 credit.",
               "inputSchema": {"type": "object", "properties": {"url": {"type": "string", "description": "The page address, starting with http:// or https://."}}, "required": ["url"]}}),
        json!({"name": MAP, "annotations": ro, "description": "List the pages of a website, to find the one you want before scraping it. Uses about 1 credit.",
               "inputSchema": {"type": "object", "properties": {"url": {"type": "string", "description": "The site's address."}, "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Most addresses to return, default 50."}}, "required": ["url"]}}),
        json!({"name": CRAWL, "annotations": ro, "description": format!("Fetch several pages of a site, starting from one address, and return each page's main text. Uses 1 credit per page, so keep the limit small (at most {MAX_CRAWL_PAGES}). Slow: prefer scrape when you know the page."),
               "inputSchema": {"type": "object", "properties": {"url": {"type": "string", "description": "Where to start."}, "limit": {"type": "integer", "minimum": 1, "maximum": MAX_CRAWL_PAGES, "description": "Most pages to fetch, default 5."}}, "required": ["url"]}}),
    ]
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

fn text_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    match args.get(key).and_then(Value::as_str).map(str::trim) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => bail!("`{key}` is required"),
    }
}

fn int_arg(args: &Value, key: &str, default: u32) -> u32 {
    args.get(key).and_then(Value::as_u64).map_or(default, |n| n.min(u32::MAX as u64) as u32)
}

/// Run one tool.
pub async fn call(client: &Client, name: &str, args: &Value) -> Result<Output> {
    match name {
        SEARCH => client.search(text_arg(args, "query")?, int_arg(args, "limit", 5)).await,
        SCRAPE | MAP | CRAWL => {
            let url = text_arg(args, "url")?;
            url_ok(url).map_err(anyhow::Error::msg)?;
            match name {
                SCRAPE => client.scrape(url).await,
                MAP => client.map(url, int_arg(args, "limit", 50)).await,
                _ => client.crawl(url, int_arg(args, "limit", 5)).await,
            }
        }
        other => bail!("there is no tool called {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_described_with_a_schema_and_marked_read_only() {
        let defs = definitions();
        assert_eq!(defs.iter().map(|d| d["name"].as_str().unwrap()).collect::<Vec<_>>(), ["web_search", "scrape", "map", "crawl"]);
        for d in &defs {
            assert_eq!(d["inputSchema"]["type"], "object");
            assert!(d["inputSchema"]["required"].as_array().is_some_and(|r| r.len() == 1));
            assert_eq!(d["annotations"]["readOnlyHint"], true, "so a CLI does not stop to ask");
            assert!(d["description"].as_str().unwrap().contains("credit"), "the agent is told what it costs");
        }
        assert_eq!(defs[3]["inputSchema"]["properties"]["limit"]["maximum"], MAX_CRAWL_PAGES);
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

    #[tokio::test]
    async fn bad_arguments_and_unknown_tools_are_errors_the_agent_can_read() {
        let c = Client::with("http://127.0.0.1:1", "k");
        let err = |name: &str, args: Value| {
            let c = &c;
            let name = name.to_owned();
            async move { call(c, &name, &args).await.unwrap_err().to_string() }
        };
        assert!(err(SEARCH, json!({})).await.contains("`query` is required"));
        assert!(err(SCRAPE, json!({"url": "  "})).await.contains("`url` is required"));
        assert!(err(SCRAPE, json!({"url": "http://localhost/x"})).await.contains("this machine"));
        assert!(err("delete_everything", json!({})).await.contains("no tool called"));
    }
}
