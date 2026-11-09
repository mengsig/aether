//! Fixtures shared by the parity and recovery suites: a started service in a
//! temporary directory, a scripted engine, and tool-call shorthands.

use serde_json::{Value, json};

use super::faux::FauxEngine;
use super::handlers::CallOutcome;
use super::records::*;
use super::*;
use crate::budget::LaneSpend;
use crate::prompts::Agent;
use crate::settings::NoesisSettings;

pub(super) const ASK: &str = "noesis.ask";
pub(super) const PROBE_DONE: &str = "noesis.probe_done";
pub(super) const PLAN: &str = "noesis.plan";
pub(super) const PROBLEM: &str = "noesis.problem";
pub(super) const CONCLUDE: &str = "noesis.conclude";

pub(super) fn start(settings: NoesisSettings) -> (tempfile::TempDir, crate::Noesis) {
    let dir = tempfile::tempdir().unwrap();
    let settings = NoesisSettings {
        enabled: true,
        ..settings
    };
    let today = chrono::Local::now().date_naive();
    let noesis =
        crate::Noesis::start_if_enabled(&settings, dir.path(), today, LaneSpend::default());
    (
        dir,
        noesis.unwrap().expect("enabled settings start the service"),
    )
}

pub(super) fn fixture() -> (tempfile::TempDir, crate::Noesis) {
    start(NoesisSettings::default())
}

pub(super) fn choice(prompt: &str, options: &[(&str, &str)], key: &[&str]) -> Value {
    let key: Vec<&str> = key
        .iter()
        .map(|value| {
            options
                .iter()
                .find(|(id, _)| id == value)
                .map_or(*value, |(_, label)| *label)
        })
        .collect();
    let options: Vec<Value> = options
        .iter()
        .map(|(v, l)| json!({"value": v, "label": l}))
        .collect();
    json!({"kind": "choice", "prompt": prompt, "options": options, "answer_key": key, "explanation": "why"})
}

pub(super) fn capital_question() -> Value {
    choice(
        "Capital of France?",
        &[("opt1", "Paris"), ("opt2", "Rome"), ("opt3", "Madrid")],
        &["opt1"],
    )
}

pub(super) fn recall(prompt: &str, key: &str) -> Value {
    json!({"kind": "recall", "prompt": prompt, "answer_key": key, "explanation": "why"})
}

pub(super) fn strands() -> Value {
    json!({"strands": [{"strand": "limits", "floor": "knows slopes", "ceiling": "epsilon-delta", "misconceptions": ["a limit is the value at the point"], "start": "limit laws"}]})
}

pub(super) fn three_steps() -> Value {
    json!({"nodes": [{"id": "a", "title": "A"}, {"id": "b", "title": "B"}, {"id": "c", "title": "C"}],
           "edges": [{"from": "a", "to": "b"}, {"from": "b", "to": "c"}]})
}

pub(super) fn session(t: &Teaching<'_, FauxEngine>, id: &str) -> SessionView {
    t.get_session(id).unwrap()
}

pub(super) fn kind_of(t: &Teaching<'_, FauxEngine>, agent: &str) -> Agent {
    t.engine().kind_of(agent).expect("a spawned agent")
}

/// A direct tool call from the session's current agent.
pub(super) fn call(
    t: &mut Teaching<'_, FauxEngine>,
    id: &str,
    name: &str,
    input: Value,
    text: &str,
) -> CallOutcome {
    let view = session(t, id);
    let role = role_of(kind_of(t, &view.agent));
    let caller = Caller {
        agent: view.agent,
        role,
    };
    let call = ToolCall {
        id: "call_direct".into(),
        name: name.into(),
        input,
    };
    t.call_tool(id, &caller, &call, text).unwrap()
}

pub(super) fn error_of(outcome: &CallOutcome) -> String {
    assert!(
        outcome.result.is_error,
        "expected a refusal, got {}",
        outcome.result.content
    );
    outcome.result.content["error"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The values the learner sees for the open question's options `labels`;
/// posing re-keys options, so tests answer by label.
pub(super) fn shown_values(t: &Teaching<'_, FauxEngine>, id: &str, labels: &[&str]) -> Vec<String> {
    let open = session(t, id).open_question.expect("an open question");
    let options = open.options.expect("a question with options");
    let values: Vec<String> = labels
        .iter()
        .map(|label| {
            let option = options.iter().find(|option| option.label == *label);
            option.expect("a label of the question").value.clone()
        })
        .collect();
    assert_eq!(values.len(), labels.len());
    values
}

/// Answer the open question with the options labelled `labels`.
pub(super) fn answer_choice(
    t: &mut Teaching<'_, FauxEngine>,
    id: &str,
    labels: &[&str],
) -> QuestionResult {
    let question = session(t, id).open_question.expect("an open question").id;
    let answer = Answer {
        choices: shown_values(t, id, labels),
        confidence: Some(70),
        ..Answer::default()
    };
    t.answer(id, &question, &answer).unwrap()
}

/// A teach session handed to its Teacher, with the three-step plan set.
pub(super) fn teacher_session(t: &mut Teaching<'_, FauxEngine>) -> String {
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut()
        .script(Agent::Prober, "", vec![(PROBE_DONE, strands())]);
    t.engine_mut().script(
        Agent::Teacher,
        "Here is the plan.",
        vec![(PLAN, three_steps())],
    );
    let reports = t.advance(&id).unwrap();
    assert_eq!(reports[0].end, TurnEnd::HandedOff);
    assert_eq!(session(t, &id).phase, Phase::Plan);
    id
}

pub(super) fn add_proposition(noesis: &crate::Noesis, id: &str, name: &str, alias: &str) {
    let conn = noesis.store().connection();
    conn.execute(
        "INSERT INTO propositions(id, name, statement, created_at) VALUES (?1, ?2, 'a true statement', 'now')",
        [id, name],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO proposition_aliases(proposition, alias) VALUES (?1, ?2)",
        [id, alias],
    )
    .unwrap();
}
