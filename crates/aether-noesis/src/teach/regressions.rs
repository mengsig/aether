//! Regressions for sealed values escaping through healed arguments, plan
//! events and session reads, and for answers owed across a restart.

use serde_json::{Value, json};

use super::faux::FauxEngine;
use super::records::*;
use super::testkit::*;
use super::*;
use crate::prompts::Agent;

const SECRET: &str = "mitochondria";

#[test]
fn a_stringified_argument_cannot_carry_an_escaped_active_solution() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise circles", Mode::Practice, &[])
        .unwrap()
        .id;
    let problem =
        json!({"statement": "Name the circle constant", "mode": "recall", "solution": "π"});
    let escaped = Value::String(r#"{"query":"\u03c0"}"#.into());
    let nested = Value::String(r#"{"query":"[\"\\u03c0\"]"}"#.into());
    let practitioner = t.engine_mut();
    practitioner.script(Agent::Practitioner, "", vec![(PROBLEM, problem)]);
    practitioner.script(
        Agent::Practitioner,
        "",
        vec![
            ("noesis.graph_search", escaped),
            ("noesis.graph_search", nested),
        ],
    );
    let report = t.advance(&id).unwrap().remove(0);
    let searches = &report.steps[1].calls;
    assert_eq!(searches.len(), 2);
    for call in searches {
        assert_eq!(call.input, json!(crate::tools::SEALED), "{call:?}");
    }
    let shown = format!("{report:?}");
    assert!(!shown.contains("u03c0") && !shown.contains('π'), "{shown}");
}

#[test]
fn a_question_whose_key_the_plan_shows_is_refused_in_either_order() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let leaky_plan = json!({"nodes": [{"id": "cell", "title": SECRET}], "edges": []});
    let mut tagged = recall("Which organelle makes ATP?", SECRET);
    tagged["plan_node"] = json!("cell");
    t.engine_mut().script(
        Agent::Teacher,
        "",
        vec![(PLAN, leaky_plan), (ASK, tagged.clone())],
    );
    t.engine_mut()
        .script(Agent::Teacher, "Let us rethink.", vec![]);
    let report = t.advance(&id).unwrap().remove(0);
    let calls = &report.steps[0].calls;
    assert!(!calls[0].is_error, "the plan alone seals nothing");
    assert!(
        calls[1].is_error,
        "the pose would expose its key: {calls:?}"
    );
    assert!(session(&t, &id).open_question.is_none());

    // The leaky plan is current; a later pose of the same key is refused too.
    t.engine_mut()
        .script(Agent::Teacher, "", vec![(ASK, tagged)]);
    t.engine_mut().script(Agent::Teacher, "Again.", vec![]);
    let report = t.advance(&id).unwrap().remove(0);
    assert!(report.steps[0].calls[0].is_error, "{report:?}");
    assert!(session(&t, &id).open_question.is_none());
}

#[test]
fn a_posed_key_never_reaches_plan_events_or_the_session_view() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    t.take_events();
    let mut tagged = recall("Which organelle makes ATP?", SECRET);
    tagged["plan_node"] = json!("a");
    t.engine_mut().script(
        Agent::Teacher,
        "",
        vec![(PLAN, three_steps()), (ASK, tagged)],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    let events = t.take_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, NoesisEvent::PlanUpdated { .. })),
        "{events:?}"
    );
    let published = serde_json::to_string(&events).unwrap();
    assert!(!published.contains(SECRET), "{published}");
    let view = format!("{:?}", session(&t, &id));
    assert!(!view.contains(SECRET), "{view}");
}

#[test]
fn an_answer_whose_delivery_failed_is_delivered_after_a_restart() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut()
        .script(Agent::Prober, "", vec![(ASK, capital_question())]);
    assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::AwaitingAnswer);
    let open = session(&t, &id).open_question.expect("posed").id;
    t.engine_mut().fail_deliveries = 1;
    assert_eq!(answer_choice(&mut t, &id, &["a"]).correct, Some(true));

    let engine = std::mem::replace(t.engine_mut(), FauxEngine::new());
    drop(t);
    let mut t = Teaching::new(&noesis, engine);
    let agent = session(&t, &id).agent;
    let answers = |t: &Teaching<'_, FauxEngine>| -> Vec<Value> {
        let inputs = t.engine().inputs_of(&agent);
        let answers = inputs.into_iter().filter_map(|input| match input {
            Input::Answer(answer) => Some(answer.clone()),
            _ => None,
        });
        answers.collect()
    };
    assert!(
        answers(&t).is_empty(),
        "the failed delivery reached nothing"
    );
    t.engine_mut().script(Agent::Prober, "Thanks.", vec![]);
    t.engine_mut().script(Agent::Prober, "Next.", vec![]);
    let steps_before = t.engine().steps_run;
    t.advance(&id).unwrap();
    assert_eq!(t.engine().steps_run, steps_before + 1);
    let delivered = answers(&t);
    assert_eq!(delivered.len(), 1, "{delivered:?}");
    assert_eq!(delivered[0]["question"], json!(open));
    assert_eq!(delivered[0]["correct"], json!(true));
    assert!(delivered[0].get("delivered").is_none(), "{delivered:?}");

    t.advance(&id).unwrap();
    assert_eq!(answers(&t).len(), 1, "an answer is delivered once");
}
