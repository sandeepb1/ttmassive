//! The plan-limit gate. Before each node starts, check how full the 5-hour and weekly windows are,
//! for the engines this flow uses. A run may use a set share of a window, and never pushes a window
//! past the stop line, so the user's own sessions are not left with nothing.
//!
//! The share is measured as how far the window has moved since the run began. Anything else using
//! the same plan in that time counts too; the windows cannot tell the two apart.

use super::graph::{Engine, Flow};
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

fn uses(flow: &Flow, engine: Engine) -> bool {
    flow.nodes.iter().any(|n| n.engine == engine)
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
pub fn pressure(run: &Run, w: &Windows, engine: Engine) -> f64 {
    let b = &run.flow.budget;
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
pub fn check(run: &Run, w: &Windows) -> Option<String> {
    let b = &run.flow.budget;
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
        assert_eq!(check(&run(), &windows(30.0, 40.0, 15.0, 20.0)), None);
    }

    #[test]
    fn the_run_stops_when_it_has_used_its_share() {
        let m = check(&run(), &windows(36.0, 40.0, 15.0, 20.0)).unwrap();
        assert!(m.contains("Claude") && m.contains("budget is 15%"), "{m}");
        let m = check(&run(), &windows(30.0, 40.0, 21.0, 20.0)).unwrap();
        assert!(m.contains("Codex") && m.contains("budget is 10%"), "{m}");
    }

    #[test]
    fn the_run_stops_at_the_stop_line_whatever_its_own_share() {
        let m = check(&run(), &windows(86.0, 40.0, 15.0, 20.0)).unwrap();
        assert!(m.contains("5-hour window is at 86%"), "{m}");
        let m = check(&run(), &windows(30.0, 90.0, 15.0, 20.0)).unwrap();
        assert!(m.contains("weekly window is at 90%"), "{m}");
    }

    #[test]
    fn unread_windows_do_not_stop_a_run() {
        assert_eq!(check(&run(), &Windows::default()), None);
        let mut r = run();
        r.baseline = Baseline::default();
        assert_eq!(check(&r, &windows(60.0, 40.0, 15.0, 20.0)), None, "no baseline: share cannot be told");
    }

    #[test]
    fn an_engine_the_flow_does_not_use_is_not_checked() {
        let mut r = run();
        r.flow.nodes.retain(|n| n.engine == Engine::Claude);
        r.flow.edges.clear();
        assert_eq!(check(&r, &windows(30.0, 40.0, 99.0, 99.0)), None);
    }

    #[test]
    fn pressure_is_the_nearer_of_the_run_share_and_the_window() {
        let r = run(); // baseline claude 20, codex 10; budgets 15 and 10; stop line 85
        // The run has used 7.5 of its 15 points: half way, though the window is far from the stop line.
        let p = pressure(&r, &windows(27.5, 30.0, 10.0, 10.0), Engine::Claude);
        assert!((p - 0.5).abs() < 1e-9, "{p}");
        // The run started high and has used nothing, but the window is near the stop line.
        let mut high = run();
        high.baseline.claude = Some(68.0);
        let p = pressure(&high, &windows(68.0, 40.0, 10.0, 10.0), Engine::Claude);
        assert!((p - 0.8).abs() < 1e-9, "{p}");
        // The weekly window counts too.
        let p = pressure(&r, &windows(20.0, 85.0, 10.0, 10.0), Engine::Claude);
        assert!((p - 1.0).abs() < 1e-9, "{p}");
        // Each engine is judged on its own windows.
        assert!(pressure(&r, &windows(80.0, 80.0, 10.0, 10.0), Engine::Codex) < 0.2);
    }

    #[test]
    fn unread_windows_mean_no_pressure() {
        assert_eq!(pressure(&run(), &Windows::default(), Engine::Claude), 0.0);
    }

    #[test]
    fn the_baseline_is_the_five_hour_fill() {
        assert_eq!(windows(12.0, 50.0, 7.0, 9.0).baseline(), Baseline { claude: Some(12.0), codex: Some(7.0) });
    }
}
