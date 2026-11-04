//! Sealing of raw engine values and recovery from failed deliveries: the
//! read paths and retry paths around the T1–T12 suite.

use serde_json::json;

use super::faux::FauxEngine;
use super::records::*;
use super::testkit::*;
use super::*;
use crate::prompts::Agent;
use crate::tools::SEALED;

const SECRET: &str = "mitochondria";

fn posed_session(t: &mut Teaching<'_, FauxEngine>) -> String {
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut()
        .script(Agent::Prober, "", vec![(ASK, capital_question())]);
    assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::AwaitingAnswer);
    id
}

#[test]
fn raw_tool_calls_and_steps_debug_only_the_sealed_projection() {
    let ask = ToolCall {
        id: "call_1".into(),
        name: ASK.into(),
        input: json!({"kind": "recall", "prompt": "Name it", "answer_key": SECRET, "explanation": "the-why"}),
    };
    let problem = ToolCall {
        id: "call_2".into(),
        name: PROBLEM.into(),
        input: json!({"statement": "Solve", "mode": "recall", "solution": "x-equals-4"}),
    };
    let step = Step {
        text: String::new(),
        calls: vec![ask.clone(), problem],
        usage: Default::default(),
    };
    let shown = format!("{ask:?} {step:?}");
    for secret in [SECRET, "the-why", "x-equals-4"] {
        assert!(!shown.contains(secret), "{secret} leaked: {shown}");
    }
    assert!(shown.contains(SEALED) && shown.contains("Name it"));
}

#[test]
fn a_pose_refused_for_giving_away_its_key_withholds_the_step_text() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let leaky = format!("Hint: it is the {SECRET}.");
    let question = recall("Name the powerhouse of the cell", SECRET);
    let malformed = json!({"kind": "riddle", "prompt": "?", "answer_key": [SECRET]});
    let prober = t.engine_mut();
    prober.script(Agent::Prober, &leaky, vec![(ASK, question.clone())]);
    prober.script(Agent::Prober, &leaky, vec![(ASK, malformed)]);
    prober.script(Agent::Prober, "One question:", vec![(ASK, question)]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    let texts: Vec<&str> = report.steps.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(texts, [WITHHELD, WITHHELD, "One question:"]);
    assert!(report.steps[0].calls[0].is_error && report.steps[1].calls[0].is_error);
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
}

#[test]
fn a_parked_session_makes_no_model_call_until_the_answer_arrives() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = posed_session(&mut t);
    t.engine_mut().script(Agent::Prober, "too early", vec![]);
    let steps_before = t.engine().steps_run;
    let again = t.advance(&id).unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].end, TurnEnd::AwaitingAnswer);
    assert!(again[0].steps.is_empty());
    assert_eq!(
        t.engine().steps_run,
        steps_before,
        "no model call while parked"
    );
    assert_eq!(t.engine().remaining(Agent::Prober), 1);
}

#[test]
fn a_failed_answer_delivery_leaves_the_question_open_for_a_retry() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = posed_session(&mut t);
    t.take_events();
    let open = session(&t, &id).open_question.expect("posed").id;
    let answer = Answer {
        choices: vec!["a".into()],
        ..Answer::default()
    };
    t.engine_mut().fail_deliveries = 1;
    let failed = t.answer(&id, &open, &answer);
    assert!(matches!(failed, Err(TeachError::Engine(_))), "{failed:?}");
    assert_eq!(
        session(&t, &id).open_question.map(|q| q.id),
        Some(open.clone())
    );
    assert!(t.take_events().is_empty() && t.take_evidence().is_empty());

    let result = t.answer(&id, &open, &answer).unwrap();
    assert_eq!(result.correct, Some(true));
    let agent = session(&t, &id).agent;
    let answers = t.engine().inputs_of(&agent);
    let delivered = answers
        .iter()
        .filter(|i| matches!(i, Input::Answer(_)))
        .count();
    assert_eq!(delivered, 1, "the answer reached the agent once");
    assert_eq!(t.take_evidence().len(), 1);
}

#[test]
fn results_kept_back_by_a_failed_delivery_arrive_before_the_answer() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let batch = vec![
        ("noesis.graph_search", json!({"query": "cell"})),
        (ASK, capital_question()),
    ];
    t.engine_mut().script(Agent::Prober, "", batch);
    t.engine_mut().fail_deliveries = 1;
    assert!(matches!(t.advance(&id), Err(TeachError::Engine(_))));
    let open = session(&t, &id)
        .open_question
        .expect("the pose committed")
        .id;

    let answer = Answer {
        choices: vec!["b".into()],
        ..Answer::default()
    };
    assert_eq!(t.answer(&id, &open, &answer).unwrap().correct, Some(false));
    let agent = session(&t, &id).agent;
    let inputs = t.engine().inputs_of(&agent);
    let tail = &inputs[inputs.len() - 2..];
    assert!(
        matches!(tail[0], Input::ToolResults(r) if r.len() == 1 && !r[0].is_error),
        "{tail:?}"
    );
    assert!(matches!(tail[1], Input::Answer(a) if a["question"] == open.as_str()));
}
