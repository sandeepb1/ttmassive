//! What the router is told about how models compare, from pages the user chose (a leaderboard, a price comparison). They
//! are fetched with the Firecrawl account at most once a day and kept on disk; if a fetch fails the last copy is used.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::web::client::Client;
use crate::secrets::Accounts;

const FILE: &str = "model-intel.json";
const DAY: i64 = 86_400;
/// Longest text kept from each page, and given to the router in all.
const PER_PAGE: usize = 3_000;
const TOTAL: usize = 6_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Intel {
    pub fetched: i64,
    /// The addresses these notes were read from: when the list changes, they are read again.
    pub urls: Vec<String>,
    pub text: String,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Whether notes read at `fetched` for `urls` still serve for `wanted` at `now`.
pub fn fresh(have: &Intel, wanted: &[String], now: i64) -> bool {
    !have.text.is_empty() && have.urls == wanted && (now - have.fetched).abs() < DAY
}

async fn read(data_dir: &Path) -> Option<Intel> {
    serde_json::from_slice(&tokio::fs::read(data_dir.join(FILE)).await.ok()?).ok()
}

/// The notes, or `None` when there are no pages chosen, no web account, or nothing could be read.
pub async fn notes(data_dir: &Path, accounts: &Accounts) -> Option<String> {
    let urls = accounts.intel_urls.clone();
    if urls.is_empty() {
        return None;
    }
    let have = read(data_dir).await.unwrap_or_default();
    if fresh(&have, &urls, now()) {
        return Some(have.text);
    }
    let client = accounts.firecrawl().and_then(|a| Client::new(a).ok());
    let mut text = String::new();
    if let Some(c) = client {
        for u in &urls {
            if let Ok(page) = c.scrape(u).await {
                text.push_str(&format!("From {u}:\n{}\n\n", crate::orch::trace::clip(&page.text, PER_PAGE)));
            }
        }
    }
    if text.is_empty() {
        // Nothing new could be read: an older copy of the same pages is better than none.
        return Some(have.text).filter(|t| !t.is_empty() && have.urls == urls);
    }
    let text = crate::orch::trace::clip(text.trim(), TOTAL);
    if let Ok(bytes) = serde_json::to_vec(&Intel { fetched: now(), urls, text: text.clone() }) {
        let _ = crate::secrets::write_private(&data_dir.join(FILE), &bytes);
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intel(fetched: i64, urls: &[&str], text: &str) -> Intel {
        Intel { fetched, urls: urls.iter().map(|u| u.to_string()).collect(), text: text.into() }
    }

    #[test]
    fn notes_serve_for_a_day_and_only_for_the_same_pages() {
        let urls = vec!["https://a.test".to_string()];
        let have = intel(1_000_000, &["https://a.test"], "notes");
        assert!(fresh(&have, &urls, 1_000_000 + 3_600));
        assert!(!fresh(&have, &urls, 1_000_000 + DAY + 1), "a day later they are read again");
        assert!(!fresh(&have, &["https://b.test".to_string()], 1_000_000), "other pages, other notes");
        assert!(!fresh(&intel(1_000_000, &["https://a.test"], ""), &urls, 1_000_000), "empty notes are not notes");
    }

    #[tokio::test]
    async fn nothing_chosen_or_nothing_readable_gives_none_and_an_old_copy_is_kept_for_the_same_pages() {
        let dir = std::env::temp_dir().join(format!("ttm-intel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = Accounts::default();
        assert_eq!(notes(&dir, &a).await, None, "no pages chosen");
        a.set_intel(vec!["https://a.test/board".into()]).unwrap();
        assert_eq!(notes(&dir, &a).await, None, "no web account and nothing kept");
        std::fs::write(dir.join(FILE), serde_json::to_vec(&intel(1, &["https://a.test/board"], "old notes")).unwrap()).unwrap();
        assert_eq!(notes(&dir, &a).await.as_deref(), Some("old notes"), "stale, but the same pages and nothing newer to be had");
        a.set_intel(vec!["https://other.test".into()]).unwrap();
        assert_eq!(notes(&dir, &a).await, None, "notes about other pages are not used");
        std::fs::remove_dir_all(dir).ok();
    }
}
