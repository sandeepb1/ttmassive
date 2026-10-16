//! What every web provider's answer is turned into, so an agent sees the same shape whichever service served it.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// When it was published, as the provider gave it ("2026-10-01", or "2 weeks ago" from some).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    /// How well it matched, for providers that rank with a score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

/// One page's main text as markdown, in full; cutting it to a size is the renderer's job.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapEntry {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SiteMap {
    pub pages: Vec<MapEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Crawl {
    pub pages: Vec<Page>,
}

/// A typed answer and the provider credits it used.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetched<T> {
    pub value: T,
    pub credits: u64,
}

/// The services a web account can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Firecrawl,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Firecrawl => "Firecrawl",
        }
    }

    pub fn can(self, _tool: Tool) -> bool {
        match self {
            Provider::Firecrawl => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Search,
    Scrape,
    Map,
    Crawl,
}

impl Tool {
    /// For "No web account could …".
    pub fn verb(self) -> &'static str {
        match self {
            Tool::Search => "search",
            Tool::Scrape => "fetch that page",
            Tool::Map => "list that site's pages",
            Tool::Crawl => "crawl that site",
        }
    }

    /// Which service to ask first, unless the order is set by hand.
    pub fn default_order(self) -> &'static [Provider] {
        &[Provider::Firecrawl]
    }
}

/// An answer from the pool: what was found, what it cost, and who served it.
#[derive(Debug, Clone, PartialEq)]
pub struct Call<T> {
    pub value: T,
    pub credits: u64,
    pub usd: f64,
    /// The service that answered; `None` when none had anything.
    pub source: Option<Provider>,
    /// Accounts passed over on the way, and why.
    pub notes: Vec<String>,
}
