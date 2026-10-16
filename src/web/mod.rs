//! Firecrawl: web search, scraping and crawling for agents. The client talks to the service with the account's key;
//! the MCP endpoint hands the same tools to agent CLIs without giving them the key.

pub mod client;
pub mod error;
pub mod mcp;
pub mod pool;
pub mod render;
pub mod serper;
pub mod tavily;
#[cfg(test)]
mod testutil;
pub mod tools;
pub mod types;
