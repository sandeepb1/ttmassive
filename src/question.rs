//! Answering Claude Code's interactive questions (the `AskUserQuestion` tool) from the chat view.
//!
//! The agent shows these as a menu in its terminal, and the only way to answer is to press keys.
//! What those keys are was worked out against the real thing (Claude Code 2.1.295):
//!
//! - Rows are numbered. Pressing a row's number picks it. With a single question that submits at
//!   once; in a form of several it records the answer and moves to the next tab.
//! - The row after the options is `Type something`: press its number, type, press Enter.
//! - In a multi-select question the numbers toggle boxes, and `Tab` moves on.
//! - After the last question of a form a review page offers `1. Submit answers`.
//!
//! The key plan is pure and tested here; running it checks the screen between steps so a
//! question that has changed or gone away is never answered blindly.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::{
    AppState,
    chat::Question,
    screen::Step,
    tmux,
};

/// How long to wait for the screen to show what a step expects.
const EXPECT_TIMEOUT: Duration = Duration::from_secs(4);
const POLL_EVERY: Duration = Duration::from_millis(150);
/// Pause between key presses: the agent drops keys that arrive in a burst.
const KEY_GAP: Duration = Duration::from_millis(200);
const MAX_TEXT: usize = 2000;

/// One answer, in the same order as the questions.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct QuestionAnswer {
    /// 1-based option numbers picked: exactly one unless the question is multi-select.
    #[serde(default)]
    pub choices: Vec<usize>,
    /// A typed answer instead of an option.
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Press a number key.
    Digit(usize),
    /// Type this text.
    Typed(String),
    Enter,
    Tab,
    /// Wait until the screen shows this text.
    Expect(String),
}

/// The key presses that answer `questions` with `answers`.
pub fn plan(questions: &[Question], answers: &[QuestionAnswer]) -> Result<Vec<Act>> {
    ensure!(!questions.is_empty(), "there is no question to answer");
    ensure!(answers.len() == questions.len(), "expected {} answers, got {}", questions.len(), answers.len());
    let mut acts = vec![Act::Expect(questions[0].question.clone())];
    for (i, (q, a)) in questions.iter().zip(answers).enumerate() {
        let n = q.options.len();
        let typed = a.text.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if let Some(text) = typed {
            ensure!(!q.multi_select, "typed answers are not supported on multiple-choice question {}", i + 1);
            ensure!(text.len() <= MAX_TEXT, "answer {} is too long", i + 1);
            ensure!(!text.contains('\n'), "answer {} must be a single line", i + 1);
            acts.extend([Act::Digit(n + 1), Act::Typed(text.to_owned()), Act::Enter]);
        } else {
            ensure!(!a.choices.is_empty(), "question {} has no answer", i + 1);
            ensure!(q.multi_select || a.choices.len() == 1, "question {} takes one answer", i + 1);
            ensure!(a.choices.iter().all(|c| (1..=n).contains(c)), "question {} has no such option", i + 1);
            let mut seen = std::collections::HashSet::new();
            ensure!(a.choices.iter().all(|c| seen.insert(*c)), "question {} repeats an option", i + 1);
            acts.extend(a.choices.iter().map(|&c| Act::Digit(c)));
            if q.multi_select {
                acts.push(Act::Tab);
            }
        }
        if let Some(next) = questions.get(i + 1) {
            acts.push(Act::Expect(next.question.clone()));
        }
    }
    // A form of several questions, or one ending in a multi-select, ends on a review page.
    let last_multi = questions.last().is_some_and(|q| q.multi_select);
    if questions.len() > 1 || last_multi {
        acts.extend([Act::Expect("Submit answers".into()), Act::Digit(1)]);
    }
    Ok(acts)
}

/// Text with all whitespace removed, so a question the terminal wrapped still matches.
fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The start of `expected`, squashed: long enough to be unique, short enough to survive truncation.
fn needle(expected: &str) -> String {
    squash(expected).chars().take(40).collect()
}

async fn wait_for(id: &str, expected: &str) -> Result<()> {
    let want = needle(expected);
    let started = tokio::time::Instant::now();
    loop {
        let screen = tmux::capture(id, 80).await?;
        if squash(&screen).contains(&want) {
            return Ok(());
        }
        if started.elapsed() > EXPECT_TIMEOUT {
            bail!("the question is no longer on screen (it may have been answered, or the agent moved on)");
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

async fn run(id: &str, acts: &[Act]) -> Result<()> {
    for act in acts {
        match act {
            Act::Expect(text) => wait_for(id, text).await?,
            Act::Digit(d) => tmux::send_steps(id, &[Step::Text(d.to_string())]).await?,
            Act::Typed(t) => tmux::send_steps(id, &[Step::Text(t.clone())]).await?,
            Act::Enter => tmux::send_steps(id, &[Step::Key("Enter")]).await?,
            Act::Tab => tmux::send_steps(id, &[Step::Key("Tab")]).await?,
        }
        tokio::time::sleep(KEY_GAP).await;
    }
    Ok(())
}

/// Answer (or, with `dismiss`, cancel) the question the agent is waiting on in `session`.
pub async fn answer(st: &Arc<AppState>, session: &str, answers: &[QuestionAnswer], dismiss: bool) -> Result<()> {
    ensure!(tmux::exists(session).await, "no such running session: {session}");
    if dismiss {
        return tmux::send_steps(session, &[Step::Key("Escape")]).await;
    }
    let cache = st.chat.clone();
    let name = session.to_owned();
    let questions = tokio::task::spawn_blocking(move || cache.pending_question(&name))
        .await
        .context("reading the conversation")?
        .context("no question is waiting for an answer")?;
    let acts = plan(&questions, answers)?;
    let _typing = st.driver.lock().await;
    run(session, &acts).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::QuestionOption;

    fn q(text: &str, multi: bool, options: &[&str]) -> Question {
        Question {
            header: "H".into(),
            question: text.into(),
            multi_select: multi,
            options: options.iter().map(|o| QuestionOption { label: (*o).into(), description: String::new() }).collect(),
        }
    }

    fn pick(c: &[usize]) -> QuestionAnswer {
        QuestionAnswer { choices: c.to_vec(), text: None }
    }

    fn typed(t: &str) -> QuestionAnswer {
        QuestionAnswer { choices: vec![], text: Some(t.into()) }
    }

    #[test]
    fn one_single_select_question_is_one_keypress() {
        let acts = plan(&[q("Which size?", false, &["Small", "Large"])], &[pick(&[2])]).unwrap();
        assert_eq!(acts, vec![Act::Expect("Which size?".into()), Act::Digit(2)]);
    }

    #[test]
    fn a_typed_answer_presses_the_type_something_row_then_types_and_confirms() {
        // Three options, so "Type something" is row 4: this is the sequence tried against Claude.
        let acts = plan(&[q("Which colour?", false, &["Red", "Green", "Blue"])], &[typed("  Teal ")]).unwrap();
        assert_eq!(
            acts,
            vec![Act::Expect("Which colour?".into()), Act::Digit(4), Act::Typed("Teal".into()), Act::Enter]
        );
    }

    #[test]
    fn a_form_waits_for_each_tab_then_submits_on_the_review_page() {
        let questions = [q("Pick a fruit?", false, &["Apple", "Pear"]), q("Which toppings?", true, &["Nuts", "Honey", "Cream"])];
        let acts = plan(&questions, &[pick(&[1]), pick(&[1, 3])]).unwrap();
        assert_eq!(
            acts,
            vec![
                Act::Expect("Pick a fruit?".into()),
                Act::Digit(1),
                Act::Expect("Which toppings?".into()),
                Act::Digit(1),
                Act::Digit(3),
                Act::Tab,
                Act::Expect("Submit answers".into()),
                Act::Digit(1),
            ]
        );
    }

    #[test]
    fn a_form_can_mix_typed_and_chosen_answers() {
        let questions = [q("A?", false, &["x", "y"]), q("B?", false, &["p"])];
        let acts = plan(&questions, &[typed("custom"), pick(&[1])]).unwrap();
        assert_eq!(
            acts,
            vec![
                Act::Expect("A?".into()),
                Act::Digit(3),
                Act::Typed("custom".into()),
                Act::Enter,
                Act::Expect("B?".into()),
                Act::Digit(1),
                Act::Expect("Submit answers".into()),
                Act::Digit(1),
            ]
        );
    }

    #[test]
    fn bad_answers_are_refused_before_any_key_is_pressed() {
        let one = [q("Q?", false, &["a", "b"])];
        let multi = [q("Q?", true, &["a", "b"])];
        let err = |r: Result<Vec<Act>>| r.unwrap_err().to_string();
        assert!(err(plan(&[], &[])).contains("no question"));
        assert!(err(plan(&one, &[])).contains("expected 1 answers"));
        assert!(err(plan(&one, &[QuestionAnswer::default()])).contains("no answer"));
        assert!(err(plan(&one, &[pick(&[3])])).contains("no such option"));
        assert!(err(plan(&one, &[pick(&[0])])).contains("no such option"));
        assert!(err(plan(&one, &[pick(&[1, 2])])).contains("takes one answer"));
        assert!(err(plan(&multi, &[pick(&[1, 1])])).contains("repeats"));
        assert!(err(plan(&multi, &[typed("x")])).contains("not supported"));
        assert!(err(plan(&one, &[typed("two\nlines")])).contains("single line"));
        assert!(err(plan(&one, &[typed(&"x".repeat(MAX_TEXT + 1))])).contains("too long"));
    }

    #[test]
    fn screen_matching_ignores_wrapping_and_spacing() {
        let wrapped = "Which colour do you\n   prefer?";
        assert!(squash(wrapped).contains(&needle("Which colour do you prefer?")));
        assert!(!squash(wrapped).contains(&needle("Which size?")));
        // Only the start is compared, so a long question cut off by the terminal still matches.
        assert_eq!(needle(&"word ".repeat(40)).len(), 40);
    }
}
