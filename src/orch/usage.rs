//! Token counts for one node or one run, and the numbers that go on the usage bar.
//! Both CLIs report the same four things; this keeps them in one shape.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    /// Input that was not served from the cache and not written to it.
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    /// What counts against a node's token cap: everything that was billed as new work.
    /// Cache reads are cheap and are left out, otherwise a long cached prefix would trip the cap.
    pub fn spent(&self) -> u64 {
        self.input + self.output + self.cache_write
    }

    /// Share of all input that came from the cache. `None` before any input has been sent.
    pub fn cache_hit(&self) -> Option<f64> {
        let input = self.input + self.cache_read + self.cache_write;
        (input > 0).then(|| self.cache_read as f64 / input as f64)
    }

    /// What was used since `base`, for engines that report a running total.
    pub fn since(&self, base: &Tokens) -> Tokens {
        Tokens {
            input: self.input.saturating_sub(base.input),
            output: self.output.saturating_sub(base.output),
            cache_read: self.cache_read.saturating_sub(base.cache_read),
            cache_write: self.cache_write.saturating_sub(base.cache_write),
        }
    }

    pub fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }

    /// Claude's `usage` object from an `assistant` message or a `result` event.
    pub fn from_claude(u: &Value) -> Tokens {
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        Tokens {
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
        }
    }

    /// Codex's `TokenUsageBreakdown`. Its `inputTokens` includes the cached and cache-write
    /// parts, so those are taken out to match the other engine. Output already includes reasoning.
    pub fn from_codex(u: &Value) -> Tokens {
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let cached = n("cachedInputTokens");
        let write = n("cacheWriteInputTokens");
        Tokens {
            input: n("inputTokens").saturating_sub(cached + write),
            output: n("outputTokens"),
            cache_read: cached,
            cache_write: write,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn claude_usage_maps_its_four_fields() {
        let t = Tokens::from_claude(&json!({
            "input_tokens": 2, "output_tokens": 140,
            "cache_read_input_tokens": 0, "cache_creation_input_tokens": 3836
        }));
        assert_eq!(t, Tokens { input: 2, output: 140, cache_read: 0, cache_write: 3836 });
        assert_eq!(t.total(), 3978);
    }

    #[test]
    fn codex_input_has_the_cached_part_taken_out() {
        let t = Tokens::from_codex(&json!({
            "inputTokens": 1000, "cachedInputTokens": 700, "cacheWriteInputTokens": 100,
            "outputTokens": 50, "reasoningOutputTokens": 20, "totalTokens": 1050
        }));
        assert_eq!(t, Tokens { input: 200, output: 50, cache_read: 700, cache_write: 100 });
    }

    #[test]
    fn cache_reads_do_not_count_against_a_cap() {
        let t = Tokens { input: 10, output: 20, cache_read: 5000, cache_write: 30 };
        assert_eq!(t.spent(), 60);
    }

    #[test]
    fn cache_hit_ratio() {
        assert_eq!(Tokens::default().cache_hit(), None);
        let t = Tokens { input: 100, output: 9, cache_read: 800, cache_write: 100 };
        assert_eq!(t.cache_hit(), Some(0.8));
    }

    #[test]
    fn since_gives_the_use_after_a_base() {
        let base = Tokens { input: 10, output: 5, cache_read: 100, cache_write: 0 };
        let now = Tokens { input: 12, output: 55, cache_read: 300, cache_write: 7 };
        assert_eq!(now.since(&base), Tokens { input: 2, output: 50, cache_read: 200, cache_write: 7 });
        assert_eq!(base.since(&now), Tokens::default());
    }

    #[test]
    fn missing_fields_count_as_zero() {
        assert_eq!(Tokens::from_claude(&json!({})), Tokens::default());
        assert_eq!(Tokens::from_codex(&json!({"inputTokens": 5})).input, 5);
    }
}
