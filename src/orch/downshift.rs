//! Cheaper models for agents that have not started yet, when the plan windows are filling up. Opt-in per
//! flow. It only ever goes down a ladder, never up, and a model it does not know is left alone.
//!
//! The choice is made once, when an agent first starts, and then kept for the rest of the run: switching
//! model part-way would start a new conversation and throw away its warm prompt cache.

use super::graph::Engine;

/// Dearest first. A model on no ladder is never changed.
const LADDERS: [&[&str]; 3] = [
    &["opus", "sonnet", "haiku"],
    &["gpt-6-astra", "gpt-6.1-sol", "gpt-6-sol", "gpt-6-luna"],
    &["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"],
];

/// From this much of the way to a limit, step down once; from the second, as far as the ladder goes.
pub const FIRST_STEP: f64 = 0.5;
pub const SECOND_STEP: f64 = 0.75;

/// How many rungs down to move for this much pressure (1.0 is the limit itself).
pub fn steps_for(pressure: f64) -> usize {
    if pressure >= SECOND_STEP {
        2
    } else if pressure >= FIRST_STEP {
        1
    } else {
        0
    }
}

/// `model` moved down its ladder by `steps`, stopping at the cheapest.
pub fn cheaper(engine: Engine, model: &str, steps: usize) -> String {
    for ladder in LADDERS {
        // A Codex model is not looked up on Claude's ladder, and the other way round.
        if (ladder[0] == "opus") != (engine == Engine::Claude) {
            continue;
        }
        if let Some(at) = ladder.iter().position(|m| *m == model) {
            return ladder[(at + steps).min(ladder.len() - 1)].to_string();
        }
    }
    model.to_string()
}

/// A cheaper model for `model` at this pressure, or `None` to keep it.
pub fn choose(engine: Engine, model: &str, pressure: f64) -> Option<String> {
    let steps = steps_for(pressure);
    if steps == 0 {
        return None;
    }
    let to = cheaper(engine, model, steps);
    (to != model).then_some(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_steps_down_once_then_all_the_way() {
        assert_eq!([0.0, 0.49, 0.5, 0.74, 0.75, 1.3].map(steps_for), [0, 0, 1, 1, 2, 2]);
    }

    #[test]
    fn claude_models_go_down_their_ladder() {
        assert_eq!(cheaper(Engine::Claude, "opus", 1), "sonnet");
        assert_eq!(cheaper(Engine::Claude, "opus", 2), "haiku");
        assert_eq!(cheaper(Engine::Claude, "sonnet", 2), "haiku");
        assert_eq!(cheaper(Engine::Claude, "haiku", 2), "haiku", "the cheapest stays put");
    }

    #[test]
    fn codex_models_go_down_theirs() {
        assert_eq!(cheaper(Engine::Codex, "gpt-6-astra", 1), "gpt-6.1-sol");
        assert_eq!(cheaper(Engine::Codex, "gpt-6.1-sol", 2), "gpt-6-luna");
        assert_eq!(cheaper(Engine::Codex, "gpt-5.6-sol", 1), "gpt-5.6-terra");
        assert_eq!(cheaper(Engine::Codex, "gpt-5.6-sol", 2), "gpt-5.6-luna");
        assert_eq!(cheaper(Engine::Codex, "gpt-6-luna", 2), "gpt-6-luna");
    }

    #[test]
    fn an_unknown_model_is_never_changed() {
        assert_eq!(cheaper(Engine::Claude, "claude-fable-5-1", 2), "claude-fable-5-1");
        assert_eq!(cheaper(Engine::Codex, "gpt-9-quasar", 2), "gpt-9-quasar");
    }

    #[test]
    fn a_name_is_only_looked_up_on_its_own_engines_ladder() {
        // "opus" typed against Codex is not a Codex model.
        assert_eq!(cheaper(Engine::Codex, "opus", 2), "opus");
        assert_eq!(cheaper(Engine::Claude, "gpt-6-astra", 2), "gpt-6-astra");
    }

    #[test]
    fn choose_says_nothing_when_nothing_would_change() {
        assert_eq!(choose(Engine::Claude, "opus", 0.3), None);
        assert_eq!(choose(Engine::Claude, "opus", 0.6).as_deref(), Some("sonnet"));
        assert_eq!(choose(Engine::Claude, "opus", 0.9).as_deref(), Some("haiku"));
        assert_eq!(choose(Engine::Claude, "haiku", 0.9), None);
    }

    #[test]
    fn it_never_goes_up() {
        for engine in [Engine::Claude, Engine::Codex] {
            for ladder in LADDERS {
                for (i, m) in ladder.iter().enumerate() {
                    for steps in 0..4 {
                        let to = cheaper(engine, m, steps);
                        if let Some(j) = ladder.iter().position(|x| *x == to) {
                            assert!(j >= i || (ladder[0] == "opus") != (engine == Engine::Claude), "{m} -> {to}");
                        }
                    }
                }
            }
        }
    }
}
