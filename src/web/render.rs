//! The text an agent reads: compact, and cut to a size that does not flood a model's context.

use super::types::{Crawl, Page, Provider, SearchHit, SiteMap};

/// Cut `s` to `max` characters, saying so when it was.
pub fn cut(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    format!("{}\n\n[… {} more characters left out]", s.chars().take(max).collect::<String>(), n - max)
}

pub fn search(hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return "No results.".into();
    }
    hits.iter()
        .enumerate()
        .map(|(i, h)| {
            let when = h.published.as_deref().map(|p| format!(" ({p})")).unwrap_or_default();
            format!("{}. {} — {}{when}\n   {}", i + 1, h.title, h.url, cut(&h.snippet, 300))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn page(p: &Page, max: usize) -> String {
    if p.markdown.is_empty() {
        return "The page had no readable text.".into();
    }
    let head = if p.title.is_empty() { p.url.clone() } else { format!("# {}\n{}", p.title, p.url) };
    format!("{head}\n\n{}", cut(&p.markdown, max))
}

pub fn map(site: &SiteMap) -> String {
    if site.pages.is_empty() {
        return "No pages found.".into();
    }
    site.pages
        .iter()
        .map(|e| match &e.title {
            Some(t) => format!("{} — {t}", e.url),
            None => e.url.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn crawl(c: &Crawl, per_page: usize, total: usize) -> String {
    if c.pages.is_empty() {
        return "The crawl found no pages.".into();
    }
    let mut out = format!("{} page(s):\n", c.pages.len());
    for p in &c.pages {
        out.push_str(&format!("\n---\n{}\n", page(p, per_page)));
        if out.chars().count() > total {
            out = cut(&out, total);
            break;
        }
    }
    out
}

/// A closing line saying who answered, and who was passed over on the way.
pub fn provenance(source: Option<Provider>, notes: &[String]) -> String {
    match (source, notes.is_empty()) {
        (Some(p), true) => format!("(via {})", p.name()),
        (Some(p), false) => format!("(via {}; {})", p.name(), notes.join("; ")),
        (None, _) => format!("(no web account had an answer: {})", notes.join("; ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::types::MapEntry;

    fn hit(title: &str, url: &str, snippet: &str) -> SearchHit {
        SearchHit { title: title.into(), url: url.into(), snippet: snippet.into(), published: None, score: None }
    }

    fn pg(title: &str, url: &str, md: &str) -> Page {
        Page { url: url.into(), title: title.into(), markdown: md.into() }
    }

    #[test]
    fn a_closing_line_names_who_answered_and_who_was_passed_over() {
        assert_eq!(provenance(Some(Provider::Firecrawl), &[]), "(via Firecrawl)");
        assert_eq!(provenance(Some(Provider::Firecrawl), &["Serper found nothing".into()]), "(via Firecrawl; Serper found nothing)");
        assert_eq!(provenance(None, &["Firecrawl found nothing".into()]), "(no web account had an answer: Firecrawl found nothing)");
    }

    #[test]
    fn answers_are_cut_and_say_so() {
        assert_eq!(cut("short", 10), "short");
        let c = cut(&"x".repeat(50), 10);
        assert!(c.starts_with("xxxxxxxxxx\n\n[… 40 more characters left out]"), "{c}");
    }

    #[test]
    fn search_results_read_as_a_numbered_list() {
        assert_eq!(search(&[hit("A", "https://a.test", "first line"), hit("B", "https://b.test", "")]), "1. A — https://a.test\n   first line\n2. B — https://b.test\n   ");
        assert_eq!(search(&[]), "No results.");
        let dated = SearchHit { published: Some("2026-10-01".into()), ..hit("N", "https://n.test", "s") };
        assert_eq!(search(&[dated]), "1. N — https://n.test (2026-10-01)\n   s");
        assert!(search(&[hit("T", "u", &"y".repeat(400))]).contains("100 more characters"), "a long snippet is cut");
    }

    #[test]
    fn pages_map_and_crawls_are_formatted_for_an_agent() {
        let home = pg("Home", "https://x.test/", "Hello world");
        assert_eq!(page(&home, 100), "# Home\nhttps://x.test/\n\nHello world");
        assert_eq!(page(&pg("", "", ""), 100), "The page had no readable text.");
        assert_eq!(page(&pg("", "https://x.test/", "Body"), 100), "https://x.test/\n\nBody", "a page with no title has no heading");
        let site = SiteMap { pages: vec![MapEntry { url: "https://x.test/a".into(), title: Some("A".into()) }, MapEntry { url: "https://x.test/b".into(), title: None }] };
        assert_eq!(map(&site), "https://x.test/a — A\nhttps://x.test/b");
        assert_eq!(map(&SiteMap { pages: vec![] }), "No pages found.");
        let c = crawl(&Crawl { pages: vec![home.clone(), home] }, 6_000, 60_000);
        assert!(c.starts_with("2 page(s):") && c.matches("# Home").count() == 2);
        assert_eq!(crawl(&Crawl { pages: vec![] }, 10, 10), "The crawl found no pages.");
        let long = Crawl { pages: vec![pg("T", "", &"x".repeat(500))] };
        assert!(crawl(&long, 100, 10_000).contains("400 more characters"), "a page is cut to its size");
        assert!(crawl(&long, 10_000, 100).contains("more characters left out"), "and the whole to its");
    }
}
