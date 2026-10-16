//! Telling a usage limit from an ordinary failure, and when it lifts. An agent that hits its plan's session or usage limit
//! says so ("You've hit your session limit · resets 6pm (Europe/London)"); that is not a fault in the work, only a wait. The
//! run parks until the limit lifts and carries on by itself.

use std::time::{SystemTime, UNIX_EPOCH};

/// Waited after the time a limit says it lifts, so the first try does not land a moment early.
pub const MARGIN_SECS: i64 = 90;
/// Waited when the message gives no time, and the shortest wait for any.
pub const FALLBACK_SECS: i64 = 15 * 60;
pub const MIN_SECS: i64 = 60;
/// The longest wait: a weekly limit can be days away, but a time further out than this is more likely misread.
pub const MAX_SECS: i64 = 8 * 24 * 3600;
/// Times one run waits for a limit before it stops and asks the user.
pub const MAX_WAITS: u32 = 12;

pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The machine's offset from UTC at `t`, in seconds. The times in these messages are in the account's time zone, which is
/// almost always the one the machine is set to; a wrong guess only makes the first try early, and it waits again.
pub fn local_offset(t: i64) -> i64 {
    // SAFETY: localtime_r only writes into the struct we give it.
    unsafe {
        let time: libc::time_t = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

/// Is this the message of an agent that has run out of plan usage? Returns it, cleaned up for showing.
pub fn detect(text: &str) -> Option<String> {
    let head: String = text.trim().chars().take(400).collect();
    let low = head.to_lowercase();
    if !low.contains("limit") {
        return None;
    }
    const SIGNS: [&str; 7] = ["resets", "reset at", "try again", "available again", "hit your", "reached your", "limit reached"];
    const KINDS: [&str; 6] = ["session limit", "usage limit", "5-hour limit", "weekly limit", "message limit", "limit reached"];
    (SIGNS.iter().any(|s| low.contains(s)) && KINDS.iter().any(|k| low.contains(k))).then(|| head.lines().next().unwrap_or("").trim().to_string())
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_year(days: i64) -> i64 {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    if mp < 10 { y } else { y + 1 }
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || matches!(c, ',' | '(' | ')' | '·' | '.')).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// A time of day from `w`, which may need `next` to say am or pm: "6pm", "6:30pm", "6" with "pm" after it, "18:00".
fn clock(w: &str, next: Option<&str>) -> Option<(i64, i64)> {
    let (body, ampm) = match w.strip_suffix("am").map(|b| (b, Some(false))).or_else(|| w.strip_suffix("pm").map(|b| (b, Some(true)))) {
        Some((b, p)) => (b, p),
        None => (w, match next {
            Some("am") => Some(false),
            Some("pm") => Some(true),
            _ => None,
        }),
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?),
        // A bare number is a time only when am or pm is said.
        None if ampm.is_some() => (body.parse::<i64>().ok()?, 0),
        None => return None,
    };
    if m > 59 || (ampm.is_none() && !w.contains(':')) {
        return None;
    }
    let h = match ampm {
        Some(pm) if (1..=12).contains(&h) => h % 12 + if pm { 12 } else { 0 },
        Some(_) => return None,
        None if h < 24 => h,
        None => return None,
    };
    Some((h, m))
}

/// A length of time from words such as "4 days 3 hours 12 minutes" or "2h 15m".
fn duration(ws: &[String]) -> Option<i64> {
    let mut total = 0i64;
    let mut found = false;
    let mut i = 0;
    while i < ws.len() {
        let w = ws[i].to_lowercase();
        let digits: String = w.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            let unit = if digits.len() < w.len() { w[digits.len()..].to_string() } else { ws.get(i + 1).map(|u| u.to_lowercase()).unwrap_or_default() };
            let n: i64 = digits.parse().ok()?;
            let secs = match unit.trim_end_matches('s') {
                "d" | "day" => 86400,
                "h" | "hr" | "hour" => 3600,
                "m" | "min" | "minute" => 60,
                "sec" | "second" => 1,
                _ => 0,
            };
            if secs > 0 {
                total += n * secs;
                found = true;
            }
        }
        i += 1;
    }
    found.then_some(total)
}

/// When the limit in `text` lifts, as unix seconds. `offset` is the local zone's offset from UTC at `now`.
pub fn reset_time(text: &str, now: i64, offset: i64) -> Option<i64> {
    let low = text.to_lowercase();
    let from = ["resets", "reset at", "try again", "available again"].iter().filter_map(|a| low.find(a).map(|i| (i, *a))).min()?;
    let tail = &low[from.0 + from.1.len()..];
    let ws = words(tail);
    // "try again in 4 hours": a length of time, counted from now.
    if matches!(ws.first().map(String::as_str), Some("in")) {
        return duration(&ws[1..]).map(|d| now + d);
    }
    let (mut month, mut day, mut time) = (None, None, None);
    for (i, w) in ws.iter().enumerate() {
        let next = ws.get(i + 1).map(String::as_str);
        if time.is_none() {
            if let Some(c) = clock(w, next) {
                time = Some(c);
                continue;
            }
        }
        if month.is_none() {
            if let Some(m) = MONTHS.iter().position(|m| w.starts_with(m)) {
                let d: Option<i64> = next.and_then(|n| n.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().ok()).filter(|d| (1..=31).contains(d));
                if d.is_some() {
                    month = Some(m as i64 + 1);
                    day = d;
                }
            }
        }
    }
    let (h, m) = time?;
    let local_now = now + offset;
    let today = local_now.div_euclid(86400);
    let at = |days: i64| days * 86400 + h * 3600 + m * 60;
    let local_target = match (month, day) {
        (Some(mo), Some(d)) => {
            let year = civil_year(today);
            let this = at(days_from_civil(year, mo, d));
            if this > local_now { this } else { at(days_from_civil(year + 1, mo, d)) }
        }
        _ => {
            let t = at(today);
            if t > local_now { t } else { t + 86400 }
        }
    };
    Some(local_target - offset)
}

/// When to try again after hitting a limit with this message, as unix seconds: a little after it says it lifts, or after a
/// fixed wait when it does not say, and never sooner than a minute or later than eight days.
pub fn retry_at(text: &str, now: i64, offset: i64) -> i64 {
    let wait = match reset_time(text, now, offset) {
        Some(t) => t - now + MARGIN_SECS,
        None => FALLBACK_SECS,
    };
    now + wait.clamp(MIN_SECS, MAX_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-16 17:57:46 in London (BST, UTC+1) is 16:57:46 UTC.
    const NOW: i64 = 1_792_169_866;
    const BST: i64 = 3600;

    fn hm(t: i64) -> (i64, i64, i64) {
        let local = t + BST;
        let secs = local.rem_euclid(86400);
        (local.div_euclid(86400) - NOW.div_euclid(86400), secs / 3600, secs % 3600 / 60)
    }

    #[test]
    fn a_session_limit_is_told_from_an_ordinary_failure() {
        assert_eq!(detect("You've hit your session limit · resets 6pm (Europe/London)").as_deref(), Some("You've hit your session limit · resets 6pm (Europe/London)"));
        assert!(detect("Claude usage limit reached. Your limit will reset at 3pm").is_some());
        assert!(detect("You've hit your usage limit. Try again at 6:12 PM.").is_some());
        assert!(detect("You've reached your weekly limit, resets Oct 20, 9am").is_some());
        assert!(detect("tool failed: no such file").is_none());
        assert!(detect("the agent did not give an answer in the required shape").is_none());
        assert!(detect("Firecrawl answered 429 Too Many Requests: rate limit exceeded").is_none(), "a tool's rate limit is not the plan's");
        assert!(detect("fix the limit in the config, then try again").is_none(), "words alone are not enough");
    }

    #[test]
    fn the_reset_time_is_read_in_the_ways_these_messages_write_it() {
        // 6pm today: it is 17:57, so a few minutes away.
        assert_eq!(hm(reset_time("You've hit your session limit · resets 6pm (Europe/London)", NOW, BST).unwrap()), (0, 18, 0));
        assert_eq!(hm(reset_time("resets 6:30pm", NOW, BST).unwrap()), (0, 18, 30));
        assert_eq!(hm(reset_time("Try again at 6:12 PM.", NOW, BST).unwrap()), (0, 18, 12));
        assert_eq!(hm(reset_time("your limit will reset at 18:00", NOW, BST).unwrap()), (0, 18, 0));
        // A time already past today is tomorrow's.
        assert_eq!(hm(reset_time("resets 5pm", NOW, BST).unwrap()), (1, 17, 0));
        assert_eq!(hm(reset_time("resets 12am", NOW, BST).unwrap()), (1, 0, 0));
        assert_eq!(hm(reset_time("resets 12pm", NOW, BST).unwrap()), (1, 12, 0));
        // With a date.
        assert_eq!(hm(reset_time("resets Oct 20, 9am (Europe/London)", NOW, BST).unwrap()), (4, 9, 0));
        assert_eq!(hm(reset_time("resets Jan 2 at 3:15pm", NOW, BST).unwrap()), (78, 15, 15), "a date earlier in the year than today is next year's");
        // A length of time.
        assert_eq!(reset_time("Try again in 4 days 3 hours 12 minutes", NOW, BST), Some(NOW + 4 * 86400 + 3 * 3600 + 12 * 60));
        assert_eq!(reset_time("try again in 2h 15m", NOW, BST), Some(NOW + 2 * 3600 + 15 * 60));
        assert_eq!(reset_time("limit reached, resets soon", NOW, BST), None);
        assert_eq!(reset_time("resets 13pm", NOW, BST), None, "not a time");
        // The zone shifts it: the same wall time is an hour earlier in UTC terms with a one-hour offset.
        assert_eq!(reset_time("resets 6pm", NOW, 0).unwrap() - reset_time("resets 6pm", NOW, BST).unwrap(), BST);
    }

    #[test]
    fn the_retry_waits_a_little_past_the_reset_and_never_too_short_or_too_long() {
        let t = retry_at("resets 6pm", NOW, BST);
        assert_eq!(hm(t - MARGIN_SECS), (0, 18, 0), "just after six, with a margin");
        assert_eq!(retry_at("you've hit your session limit", NOW, BST), NOW + FALLBACK_SECS, "no time given");
        assert!(retry_at("Try again in 2 seconds", NOW, BST) >= NOW + MIN_SECS, "never sooner than a minute");
        assert_eq!(retry_at("Try again in 2 seconds", NOW, BST), NOW + 2 + MARGIN_SECS);
        assert_eq!(retry_at("try again in 40 days", NOW, BST), NOW + MAX_SECS, "never beyond eight days");
    }

    #[test]
    fn the_local_offset_is_a_whole_number_of_minutes() {
        assert_eq!(local_offset(NOW) % 60, 0);
        assert!(local_offset(NOW).abs() <= 14 * 3600);
    }
}
