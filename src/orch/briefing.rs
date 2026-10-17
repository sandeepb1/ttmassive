//! What a designer is told about everything an agent could run on: your Claude and Codex plans and how full their windows are,
//! every account with its models, prices and balance, the web services, and what a run is allowed to spend. Built from plain
//! facts so it can be tested without asking anything, and written for a model to read: figures first, then what they mean.

use super::graph::Budget;

/// One of your plans (Claude Code or Codex) and how much of it has been used.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Percent of the 5-hour and 7-day windows used, when known.
    pub five_hour: Option<f64>,
    pub seven_day: Option<f64>,
    pub models: Vec<PlanModel>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanModel {
    pub id: String,
    /// What it is good for.
    pub note: String,
    /// The reasoning efforts it supports, lightest first; empty when it does not say.
    pub efforts: Vec<String>,
}

/// A model on an account that is paid for per token.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountModel {
    pub id: String,
    /// The agent CLI that runs it.
    pub engine: &'static str,
    /// Dollars per million tokens.
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
    pub tools: bool,
    pub context: Option<u64>,
    pub reasoning: Option<bool>,
    /// What the account's catalog says about it.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub name: String,
    /// What is left on it, in words, when it says.
    pub balance: Option<String>,
    pub models: Vec<AccountModel>,
}

/// What agents can be given on the web.
#[derive(Debug, Clone, PartialEq)]
pub struct Web {
    pub tools: Vec<&'static str>,
    pub services: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Briefing {
    pub claude: Plan,
    pub codex: Plan,
    pub accounts: Vec<Account>,
    pub web: Option<Web>,
    pub limits: Budget,
    /// Pages the user chose about how models compare, trimmed.
    pub intel: Option<String>,
}

/// What each Claude model is for, as Claude Code's own models are described to a person choosing among them.
pub fn claude_models() -> Vec<PlanModel> {
    let m = |id: &str, note: &str| PlanModel { id: id.into(), note: note.into(), efforts: ["low", "medium", "high", "xhigh", "max"].map(String::from).to_vec() };
    vec![
        m("opus", "the strongest reasoning and planning, and the slowest and costliest on your plan: for the few agents whose mistakes are expensive"),
        m("sonnet", "strong at coding and analysis for a moderate cost: the workhorse for most agents"),
        m("haiku", "fast and cheap: for searching, summarising and simple checks"),
    ]
}

/// The most models listed for one account, so a long catalog does not drown the rest.
const MAX_PER_ACCOUNT: usize = 25;

fn pct(p: f64) -> String {
    format!("{}%", p.round() as i64)
}

fn price(p: Option<f64>) -> String {
    p.map_or("?".into(), |p| if p == 0.0 { "free".into() } else { format!("${p}") })
}

fn plan_usage(name: &str, p: &Plan) -> String {
    match (p.five_hour, p.seven_day) {
        (None, None) => format!("{name}: usage is not known."),
        (a, b) => {
            let part = |label: &str, v: Option<f64>| v.map(|v| format!("{} of the {label} window", pct(v)));
            format!("{name}: {} used.", [part("5-hour", a), part("7-day", b)].into_iter().flatten().collect::<Vec<_>>().join(", "))
        }
    }
}

/// What the figures mean for the choice of models: the part a designer would otherwise have to work out, or not notice.
pub fn guidance(b: &Briefing) -> Vec<String> {
    let mut out = Vec::new();
    let level = |p: &Plan| p.five_hour.unwrap_or(0.0).max(p.seven_day.unwrap_or(0.0));
    let (c, x) = (level(&b.claude), level(&b.codex));
    let claude_here = !b.claude.models.is_empty();
    let codex_here = !b.codex.models.is_empty();
    let paid = b.accounts.iter().any(|a| !a.models.is_empty());
    let note = |name: &str, used: f64, others: &str| -> Option<String> {
        if used >= 80.0 {
            Some(format!("{name} is {} used and nearly full: give it as few agents as you can, the cheapest models that do the job, and put the rest on {others}.", pct(used)))
        } else if used >= 50.0 {
            Some(format!("{name} is {} used: keep its strongest model for the one or two agents that need it, and spread the rest.", pct(used)))
        } else {
            None
        }
    };
    if claude_here {
        out.extend(note("Your Claude plan", c, "Codex or paid models"));
    }
    if codex_here {
        out.extend(note("Your Codex plan", x, "Claude or paid models"));
    }
    if (!claude_here || c >= 80.0) && (!codex_here || x >= 80.0) {
        out.push(if paid {
            "Neither plan has room to spare, so put most agents on the paid models below, choosing the cheapest that fit each job.".into()
        } else {
            "Neither plan has room to spare and there are no paid models: keep the team as small as the work allows.".into()
        });
    }
    if !claude_here {
        out.push("Claude Code is not available: do not give any agent a Claude model.".into());
    }
    if !codex_here {
        out.push("Codex is not available: do not give any agent a Codex model.".into());
    }
    if let Some(cap) = b.limits.run_usd {
        out.push(format!("A run may spend at most ${cap} on paid models, then it stops: choose models and the number of agents so the work fits."));
    }
    if let Some(cap) = b.limits.run_web_usd {
        out.push(format!("A run may spend at most ${cap} on web access."));
    }
    out.push(format!(
        "A run stops starting new agents when any plan window reaches {}, and an agent may use at most {} of your Claude and {} of your Codex 5-hour windows.",
        pct(b.limits.stop_window_pct),
        pct(b.limits.claude_pct),
        pct(b.limits.codex_pct)
    ));
    out
}

/// The briefing as text for a model to read.
pub fn render(b: &Briefing) -> String {
    let mut s = String::from("# What you can use\n\n");
    s.push_str("## Your Claude Code plan\n\n");
    if b.claude.models.is_empty() {
        s.push_str("Not available.\n\n");
    } else {
        s.push_str(&plan_usage("Usage", &b.claude));
        s.push_str("\n\nModels (the engine is claude):\n");
        for m in &b.claude.models {
            s.push_str(&format!("- {}: {}\n", m.id, m.note));
        }
        s.push('\n');
    }
    s.push_str("## Your Codex plan\n\n");
    if b.codex.models.is_empty() {
        s.push_str("Not available.\n\n");
    } else {
        s.push_str(&plan_usage("Usage", &b.codex));
        s.push_str("\n\nModels (the engine is codex):\n");
        for m in &b.codex.models {
            let efforts = if m.efforts.is_empty() { String::new() } else { format!(" Reasoning effort: {}.", m.efforts.join(", ")) };
            let note = if m.note.is_empty() { "no description" } else { m.note.as_str() };
            s.push_str(&format!("- {}: {note}{efforts}\n", m.id));
        }
        s.push('\n');
    }
    for a in &b.accounts {
        s.push_str(&format!("## {} (paid per token)\n\n", a.name));
        if let Some(bal) = &a.balance {
            s.push_str(&format!("Balance: {bal}.\n\n"));
        }
        if a.models.is_empty() {
            s.push_str("No models are pinned for agents.\n\n");
            continue;
        }
        s.push_str("Models an agent can be given (set the model to its id; the engine is the agent CLI that runs it):\n");
        for m in a.models.iter().take(MAX_PER_ACCOUNT) {
            let mut facts = vec![format!("engine {}", m.engine), format!("{} in / {} out per million tokens", price(m.price_in), price(m.price_out))];
            if let Some(c) = m.context.filter(|c| *c > 0) {
                facts.push(if c >= 1_000_000 { format!("{}M context", (c as f64 / 1e6 * 10.0).round() / 10.0) } else { format!("{}k context", c / 1000) });
            }
            if m.reasoning == Some(true) {
                facts.push("reasoning".into());
            }
            facts.push(if m.tools { "tool use".into() } else { "no tool use".into() });
            let note = if m.note.is_empty() { String::new() } else { format!(". {}", m.note) };
            s.push_str(&format!("- {} ({}){note}\n", m.id, facts.join("; ")));
        }
        if a.models.len() > MAX_PER_ACCOUNT {
            s.push_str(&format!("- …and {} more.\n", a.models.len() - MAX_PER_ACCOUNT));
        }
        s.push('\n');
    }
    if b.accounts.is_empty() {
        s.push_str("## Paid models\n\nNone are set up, so every agent runs on your plans.\n\n");
    }
    s.push_str("## Web access\n\n");
    match &b.web {
        Some(w) => s.push_str(&format!("Agents can be given web tools ({}) through {}. Each call costs credits, counted against a run's web cap. Turn it on only for agents that look things up.\n\n", w.tools.join(", "), w.services.join(", "))),
        None => s.push_str("No web service is set up, so no agent can search or read pages.\n\n"),
    }
    if let Some(intel) = b.intel.as_deref().filter(|t| !t.trim().is_empty()) {
        s.push_str("## How models compare (from pages the user chose)\n\n");
        s.push_str(intel.trim());
        s.push_str("\n\n");
    }
    s.push_str("## What this means for your choices\n\n");
    for g in guidance(b) {
        s.push_str(&format!("- {g}\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(five: Option<f64>, seven: Option<f64>, models: &[&str]) -> Plan {
        Plan { five_hour: five, seven_day: seven, models: models.iter().map(|m| PlanModel { id: (*m).into(), note: format!("{m} note"), efforts: vec![] }).collect() }
    }

    fn paid() -> Account {
        Account {
            name: "OpenRouter".into(),
            balance: Some("$9.12 left of $10.00".into()),
            models: vec![
                AccountModel { id: "anthropic/claude-haiku-5.5".into(), engine: "claude", price_in: Some(0.8), price_out: Some(4.0), tools: true, context: Some(200_000), reasoning: Some(true), note: "Fast and cheap.".into() },
                AccountModel { id: "x/free".into(), engine: "codex", price_in: Some(0.0), price_out: Some(0.0), tools: false, context: Some(1_500_000), reasoning: None, note: String::new() },
            ],
        }
    }

    #[test]
    fn every_account_model_and_window_is_written_down_with_its_facts() {
        let b = Briefing { claude: plan(Some(46.4), Some(18.0), &["opus", "sonnet"]), codex: plan(Some(0.0), None, &["gpt-6-luna"]), accounts: vec![paid()], web: Some(Web { tools: vec!["web_search", "scrape"], services: vec!["Serper".into(), "Tavily".into()] }), ..Default::default() };
        let t = render(&b);
        assert!(t.contains("Usage: 46% of the 5-hour window, 18% of the 7-day window used."), "{t}");
        assert!(t.contains("- opus: opus note") && t.contains("- gpt-6-luna: gpt-6-luna note"));
        assert!(t.contains("Usage: 0% of the 5-hour window used."), "a window that is not known is left out");
        assert!(t.contains("## OpenRouter (paid per token)") && t.contains("Balance: $9.12 left of $10.00."));
        assert!(t.contains("- anthropic/claude-haiku-5.5 (engine claude; $0.8 in / $4 out per million tokens; 200k context; reasoning; tool use). Fast and cheap."), "{t}");
        assert!(t.contains("- x/free (engine codex; free in / free out per million tokens; 1.5M context; no tool use)"), "{t}");
        assert!(t.contains("web_search, scrape") && t.contains("Serper, Tavily"));
    }

    #[test]
    fn an_unknown_plan_and_a_missing_service_are_said_so_rather_than_left_out() {
        let b = Briefing { claude: plan(None, None, &["sonnet"]), ..Default::default() };
        let t = render(&b);
        assert!(t.contains("Usage: usage is not known.") || t.contains("usage is not known"), "{t}");
        assert!(t.contains("## Your Codex plan\n\nNot available."));
        assert!(t.contains("None are set up, so every agent runs on your plans."));
        assert!(t.contains("No web service is set up"));
        assert!(!t.contains("How models compare"), "no pages, no section");
    }

    #[test]
    fn a_nearly_full_plan_is_said_to_be_nearly_full_and_what_to_do_about_it() {
        let b = Briefing { claude: plan(Some(91.0), Some(30.0), &["opus"]), codex: plan(Some(10.0), Some(5.0), &["gpt-6-luna"]), ..Default::default() };
        let g = guidance(&b);
        assert!(g.iter().any(|l| l.contains("Your Claude plan is 91% used and nearly full") && l.contains("Codex or paid models")), "{g:?}");
        assert!(!g.iter().any(|l| l.contains("Your Codex plan is")), "a plan with room is not warned about");
        let half = guidance(&Briefing { claude: plan(Some(55.0), None, &["opus"]), codex: plan(Some(1.0), None, &["x"]), ..Default::default() });
        assert!(half.iter().any(|l| l.contains("55% used: keep its strongest model")), "{half:?}");
        let calm = guidance(&Briefing { claude: plan(Some(10.0), Some(10.0), &["opus"]), codex: plan(Some(5.0), None, &["x"]), ..Default::default() });
        assert!(!calm.iter().any(|l| l.contains("used")), "no pressure, no warning: {calm:?}");
    }

    #[test]
    fn when_neither_plan_has_room_it_says_where_to_go_instead() {
        let full = Briefing { claude: plan(Some(95.0), None, &["opus"]), codex: plan(Some(88.0), None, &["x"]), accounts: vec![paid()], ..Default::default() };
        assert!(guidance(&full).iter().any(|l| l.contains("Neither plan has room to spare, so put most agents on the paid models")));
        let nothing = Briefing { claude: plan(Some(95.0), None, &["opus"]), codex: plan(Some(88.0), None, &["x"]), ..Default::default() };
        assert!(guidance(&nothing).iter().any(|l| l.contains("no paid models: keep the team as small")));
    }

    #[test]
    fn an_engine_that_is_not_there_is_ruled_out_by_name() {
        let only_codex = Briefing { codex: plan(Some(0.0), None, &["x"]), ..Default::default() };
        let g = guidance(&only_codex);
        assert!(g.iter().any(|l| l == "Claude Code is not available: do not give any agent a Claude model."), "{g:?}");
        assert!(!g.iter().any(|l| l.contains("Codex is not available")));
    }

    #[test]
    fn the_limits_of_a_run_are_part_of_what_it_is_told() {
        let b = Briefing { claude: plan(None, None, &["sonnet"]), limits: Budget { run_usd: Some(2.5), run_web_usd: Some(0.25), ..Default::default() }, ..Default::default() };
        let t = render(&b);
        assert!(t.contains("at most $2.5 on paid models") && t.contains("at most $0.25 on web access"), "{t}");
        assert!(t.contains("any plan window reaches 85%") && t.contains("15% of your Claude and 10% of your Codex"), "{t}");
        assert!(!render(&Briefing::default()).contains("on paid models, then it stops"), "no cap, no sentence about one");
    }

    #[test]
    fn a_long_catalog_is_cut_and_says_how_many_were_left_out() {
        let many = Account { name: "Big".into(), balance: None, models: (0..40).map(|i| AccountModel { id: format!("v/m{i}"), engine: "codex", price_in: None, price_out: None, tools: true, context: None, reasoning: None, note: String::new() }).collect() };
        let t = render(&Briefing { accounts: vec![many], ..Default::default() });
        assert!(t.contains("v/m24") && !t.contains("v/m25"), "{t}");
        assert!(t.contains("…and 15 more."));
        assert!(t.contains("$?") == false && t.contains("? in / ? out"), "an unknown price is a question mark");
    }

    #[test]
    fn the_pages_the_user_chose_about_how_models_compare_are_included_trimmed() {
        let t = render(&Briefing { intel: Some("\n  From https://a.test: haiku beats x on speed.\n".into()), ..Default::default() });
        assert!(t.contains("## How models compare (from pages the user chose)\n\nFrom https://a.test: haiku beats x on speed.\n"), "{t}");
        assert!(!render(&Briefing { intel: Some("   ".into()), ..Default::default() }).contains("How models compare"));
    }

    #[test]
    fn claude_models_each_say_what_they_are_for() {
        let m = claude_models();
        assert_eq!(m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["opus", "sonnet", "haiku"]);
        assert!(m.iter().all(|m| !m.note.is_empty() && m.efforts.len() == 5));
    }
}
