//! The plan-limit gate. Before each node starts, check how full the 5-hour and weekly windows are,
//! for the engines this flow uses. A run may use a set share of a window, and never pushes a window
//! past the stop line, so the user's own sessions are not left with nothing.
//!
//! The share is measured as how far the window has moved since the run began. Anything else using
//! the same plan in that time counts too; the windows cannot tell the two apart.

use super::graph::{Budget, Engine, Flow};
use super::run::{Baseline, Run};

/// Window fill, 0 to 100, for each engine. `None` when it has not been read.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Windows {
    pub claude_5h: Option<f64>,
    pub claude_7d: Option<f64>,
    pub codex_5h: Option<f64>,
    pub codex_7d: Option<f64>,
}

impl Windows {
    pub fn from_snapshot(s: &crate::usage::Snapshot) -> Windows {
        Windows {
            claude_5h: s.claude.five_hour.map(|w| w.used_pct),
            claude_7d: s.claude.seven_day.map(|w| w.used_pct),
            codex_5h: s.codex.five_hour.map(|w| w.used_pct),
            codex_7d: s.codex.seven_day.map(|w| w.used_pct),
        }
    }

    pub fn baseline(&self) -> Baseline {
        Baseline { claude: self.claude_5h, codex: self.codex_5h }
    }
}

/// Dollars with enough digits to tell small amounts apart: $2.50, but $0.0004.
fn usd(n: f64) -> String {
    if n >= 0.01 { format!("${n:.2}") } else { format!("${n:.4}") }
}

/// Whether any agent of the flow draws on `engine`'s plan. Agents on OpenRouter do not.
fn uses(flow: &Flow, engine: Engine) -> bool {
    flow.nodes.iter().any(|n| n.engine == engine && n.uses_plan())
}

fn check_engine(
    name: &str,
    five: Option<f64>,
    week: Option<f64>,
    base: Option<f64>,
    budget_pct: f64,
    stop_pct: f64,
) -> Option<String> {
    for (label, v) in [("5-hour", five), ("weekly", week)] {
        if let Some(v) = v.filter(|v| *v >= stop_pct) {
            return Some(format!("The {name} {label} window is at {v:.0}% (the run stops at {stop_pct:.0}%)"));
        }
    }
    if let (Some(now), Some(start)) = (five, base) {
        let used = now - start;
        if used >= budget_pct {
            return Some(format!(
                "This run has used about {used:.0}% of the {name} 5-hour window (its budget is {budget_pct:.0}%)"
            ));
        }
    }
    None
}

/// How close this run is to stopping on `engine`'s windows: 0 is nothing used, 1 is a limit reached. Either the
/// run's own share against its budget, or a whole window against the stop line, whichever is closer.
pub fn pressure(run: &Run, w: &Windows, b: &Budget, engine: Engine) -> f64 {
    let (five, week, base, budget) = match engine {
        Engine::Claude => (w.claude_5h, w.claude_7d, run.baseline.claude, b.claude_pct),
        Engine::Codex => (w.codex_5h, w.codex_7d, run.baseline.codex, b.codex_pct),
    };
    let mut p: f64 = 0.0;
    for v in [five, week].into_iter().flatten() {
        p = p.max(v / b.stop_window_pct);
    }
    if let (Some(now), Some(start)) = (five, base) {
        p = p.max((now - start).max(0.0) / budget);
    }
    p
}

/// A reason to stop before the next node, or `None` to go on.
pub fn check(run: &Run, w: &Windows, b: &Budget) -> Option<String> {
    if run.waived {
        return None;
    }
    if let Some(cap) = b.run_web_usd {
        let spent = run.web_usd();
        if spent >= cap {
            return Some(format!("This run has spent {} on web access (its cap is {})", usd(spent), usd(cap)));
        }
    }
    if let Some(cap) = b.run_usd {
        let spent = run.cost_usd();
        if spent >= cap {
            return Some(format!("This run has spent {} on OpenRouter (its dollar cap is {})", usd(spent), usd(cap)));
        }
    }
    if uses(&run.flow, Engine::Claude) {
        if let Some(m) = check_engine("Claude", w.claude_5h, w.claude_7d, run.baseline.claude, b.claude_pct, b.stop_window_pct) {
            return Some(m);
        }
    }
    if uses(&run.flow, Engine::Codex) {
        if let Some(m) = check_engine("Codex", w.codex_5h, w.codex_7d, run.baseline.codex, b.codex_pct, b.stop_window_pct) {
            return Some(m);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orch::starters;

    fn run() -> Run {
        // First starter: Claude and Codex nodes; budget 15% Claude, 10% Codex, stop at 85%.
        let mut r = Run::new("r".into(), starters::all().remove(0), "g".into(), 0);
        r.baseline = Baseline { claude: Some(20.0), codex: Some(10.0) };
        r
    }

    fn windows(c5: f64, c7: f64, x5: f64, x7: f64) -> Windows {
        Windows { claude_5h: Some(c5), claude_7d: Some(c7), codex_5h: Some(x5), codex_7d: Some(x7) }
    }

    #[test]
    fn well_inside_every_limit_goes_on() {
        assert_eq!(check(&run(), &windows(30.0, 40.0, 15.0, 20.0), &Budget::default()), None);
    }

    #[test]
    fn the_run_stops_when_it_has_used_its_share() {
        let m = check(&run(), &windows(36.0, 40.0, 15.0, 20.0), &Budget::default()).unwrap();
        assert!(m.contains("Claude") && m.contains("budget is 15%"), "{m}");
        let m = check(&run(), &windows(30.0, 40.0, 21.0, 20.0), &Budget::default()).unwrap();
        assert!(m.contains("Codex") && m.contains("budget is 10%"), "{m}");
    }

    #[test]
    fn the_run_stops_at_the_stop_line_whatever_its_own_share() {
        let m = check(&run(), &windows(86.0, 40.0, 15.0, 20.0), &Budget::default()).unwrap();
        assert!(m.contains("5-hour window is at 86%"), "{m}");
        let m = check(&run(), &windows(30.0, 90.0, 15.0, 20.0), &Budget::default()).unwrap();
        assert!(m.contains("weekly window is at 90%"), "{m}");
    }

    #[test]
    fn unread_windows_do_not_stop_a_run() {
        assert_eq!(check(&run(), &Windows::default(), &Budget::default()), None);
        let mut r = run();
        r.baseline = Baseline::default();
        assert_eq!(check(&r, &windows(60.0, 40.0, 15.0, 20.0), &Budget::default()), None, "no baseline: share cannot be told");
    }

    #[test]
    fn an_engine_the_flow_does_not_use_is_not_checked() {
        let mut r = run();
        r.flow.nodes.retain(|n| n.engine == Engine::Claude);
        r.flow.edges.clear();
        assert_eq!(check(&r, &windows(30.0, 40.0, 99.0, 99.0), &Budget::default()), None);
    }

    #[test]
    fn pressure_is_the_nearer_of_the_run_share_and_the_window() {
        let r = run(); // baseline claude 20, codex 10; budgets 15 and 10; stop line 85
        // The run has used 7.5 of its 15 points: half way, though the window is far from the stop line.
        let p = pressure(&r, &windows(27.5, 30.0, 10.0, 10.0), &Budget::default(), Engine::Claude);
        assert!((p - 0.5).abs() < 1e-9, "{p}");
        // The run started high and has used nothing, but the window is near the stop line.
        let mut high = run();
        high.baseline.claude = Some(68.0);
        let p = pressure(&high, &windows(68.0, 40.0, 10.0, 10.0), &Budget::default(), Engine::Claude);
        assert!((p - 0.8).abs() < 1e-9, "{p}");
        // The weekly window counts too.
        let p = pressure(&r, &windows(20.0, 85.0, 10.0, 10.0), &Budget::default(), Engine::Claude);
        assert!((p - 1.0).abs() < 1e-9, "{p}");
        // Each engine is judged on its own windows.
        assert!(pressure(&r, &windows(80.0, 80.0, 10.0, 10.0), &Budget::default(), Engine::Codex) < 0.2);
    }

    #[test]
    fn unread_windows_mean_no_pressure() {
        assert_eq!(pressure(&run(), &Windows::default(), &Budget::default(), Engine::Claude), 0.0);
    }

    #[test]
    fn the_limits_are_whatever_is_set_now_not_what_the_flow_was_saved_with() {
        let r = run();
        let w = windows(36.0, 40.0, 15.0, 20.0); // Claude used 16 points: over 15, under 25
        assert!(check(&r, &w, &Budget::default()).is_some());
        assert_eq!(check(&r, &w, &Budget { claude_pct: 25.0, ..Budget::default() }), None);
        assert!(check(&r, &windows(60.0, 40.0, 15.0, 20.0), &Budget { stop_window_pct: 50.0, ..Budget::default() }).is_some());
    }

    #[test]
    fn a_waived_run_is_never_stopped_by_the_limits() {
        let mut r = run();
        assert!(check(&r, &windows(99.0, 99.0, 99.0, 99.0), &Budget::default()).is_some());
        r.waived = true;
        assert_eq!(check(&r, &windows(99.0, 99.0, 99.0, 99.0), &Budget::default()), None);
    }

    fn on_openrouter(r: &mut Run) {
        for n in &mut r.flow.nodes {
            n.account = Some("acct".into());
        }
    }

    #[test]
    fn a_flow_that_runs_only_on_openrouter_is_not_held_to_the_plan_windows() {
        let mut r = run();
        assert!(check(&r, &windows(99.0, 99.0, 99.0, 99.0), &Budget::default()).is_some());
        on_openrouter(&mut r);
        assert_eq!(check(&r, &windows(99.0, 99.0, 99.0, 99.0), &Budget::default()), None, "no agent draws on either plan");
        // One agent back on the plan brings that plan's check back, for its engine only.
        let (id, engine) = (r.flow.nodes[0].id.clone(), r.flow.nodes[0].engine);
        r.flow.nodes.iter_mut().find(|n| n.id == id).unwrap().account = None;
        let w = windows(99.0, 99.0, 10.0, 10.0);
        assert_eq!(check(&r, &w, &Budget::default()).is_some(), engine == Engine::Claude);
    }

    #[test]
    fn a_run_stops_when_it_has_spent_its_dollar_cap_and_not_before() {
        let mut r = run();
        on_openrouter(&mut r);
        let ids: Vec<String> = r.flow.nodes.iter().map(|n| n.id.clone()).collect();
        r.nodes.get_mut(&ids[0]).unwrap().cost_usd = 0.40;
        r.nodes.get_mut(&ids[1]).unwrap().cost_usd = 0.35;
        let cap = |c: f64| Budget { run_usd: Some(c), ..Budget::default() };
        assert_eq!(check(&r, &Windows::default(), &cap(1.00)), None);
        let m = check(&r, &Windows::default(), &cap(0.75)).expect("0.75 spent is at the cap");
        assert!(m.contains("$0.75") && m.contains("dollar cap is $0.75"), "{m}");
        assert_eq!(usd(0.0004), "$0.0004");
        assert_eq!(usd(2.5), "$2.50");
        assert_eq!(check(&r, &Windows::default(), &Budget::default()), None, "no cap, no stop");
        r.waived = true;
        assert_eq!(check(&r, &Windows::default(), &cap(0.10)), None, "carrying on anyway skips the cap");
    }

    #[test]
    fn a_run_stops_when_its_web_tools_have_spent_its_cost_cap() {
        let mut r = run();
        on_openrouter(&mut r);
        let ids: Vec<String> = r.flow.nodes.iter().map(|n| n.id.clone()).collect();
        r.nodes.get_mut(&ids[0]).unwrap().web_usd = 0.75;
        r.nodes.get_mut(&ids[1]).unwrap().web_usd = 0.5;
        r.research_web_usd = 0.25;
        let cap = |c: f64| Budget { run_web_usd: Some(c), ..Budget::default() };
        assert_eq!(r.web_usd(), 1.5);
        assert_eq!(check(&r, &Windows::default(), &cap(1.75)), None);
        let m = check(&r, &Windows::default(), &cap(1.5)).expect("at the cap");
        assert!(m.contains("$1.50 on web access") && m.contains("cap is $1.50"), "{m}");
        assert_eq!(check(&r, &Windows::default(), &Budget::default()), None, "no cap, no stop");
        for bad in [0.0, -1.0, f64::NAN] {
            assert!(cap(bad).validate().is_err(), "a cap of {bad} would stop every run");
        }
        r.waived = true;
        assert_eq!(check(&r, &Windows::default(), &cap(0.01)), None);
    }

    #[test]
    fn the_baseline_is_the_five_hour_fill() {
        assert_eq!(windows(12.0, 50.0, 7.0, 9.0).baseline(), Baseline { claude: Some(12.0), codex: Some(7.0) });
    }
}
