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
    assert_eq!(answer_choice(&mut t, &id, &["Paris"]).correct, Some(true));

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

#[test]
fn the_models_option_values_never_reach_a_reader_so_text_cannot_name_the_key() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    t.take_events();
    let named = "The correct option value is option-a.";
    let leaky_plan = json!({"nodes": [{"id": "pick", "title": named}], "edges": []});
    let pose = |plan_node: Option<&str>| {
        let mut pose = choice(
            "Capital of France?",
            &[("option-a", "Paris"), ("option-b", "Rome")],
            &["option-a"],
        );
        if let Some(node) = plan_node {
            pose["plan_node"] = json!(node);
        }
        pose
    };
    t.engine_mut().script(
        Agent::Teacher,
        "",
        vec![(PLAN, leaky_plan), (ASK, pose(Some("pick")))],
    );
    t.engine_mut().script(Agent::Teacher, "Rethinking.", vec![]);
    let report = t.advance(&id).unwrap().remove(0);
    assert!(report.steps[0].calls[1].is_error, "{report:?}");
    assert!(session(&t, &id).open_question.is_none());

    let clean = json!({"nodes": [{"id": "pick", "title": "Capitals"}], "edges": []});
    t.engine_mut().script(
        Agent::Teacher,
        "",
        vec![(PLAN, clean), (ASK, pose(Some("pick")))],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer, "{report:?}");
    let events = serde_json::to_string(&t.take_events()).unwrap();
    let view = serde_json::to_string(&session(&t, &id)).unwrap();
    let shown = format!("{events}{view}");
    assert!(shown.contains("Paris") && shown.contains("Rome"), "{shown}");
    for value in ["\"option-a\"", "\"option-b\""] {
        assert!(!shown.contains(value), "{value} reached a reader: {shown}");
    }
    let mut values = shown_values(&t, &id, &["Paris", "Rome"]);
    values.sort();
    assert_eq!(values, ["1", "2"], "options show only display positions");

    let result = answer_choice(&mut t, &id, &["Paris"]);
    assert_eq!(result.correct, Some(true));
    let agent = session(&t, &id).agent;
    let inputs = t.engine().inputs_of(&agent);
    let Some(Input::Answer(delivered)) = inputs.last() else {
        panic!("the answer reaches the agent: {inputs:?}");
    };
    assert_eq!(delivered["chosen"], json!(["Paris"]), "{delivered}");
}

#[test]
fn a_choice_whose_prompt_or_labels_show_an_option_value_is_refused() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let labelled = choice(
        "Capital of France?",
        &[
            ("option-a", "option-a: Paris"),
            ("option-b", "option-b: Rome"),
        ],
        &["option-a"],
    );
    let listed = choice(
        "Capital of France: (opt1) Paris or (opt2) Rome?",
        &[("opt1", "Paris"), ("opt2", "Rome")],
        &["opt1"],
    );
    for pose in [labelled, listed] {
        let error = error_of(&call(&mut t, &id, ASK, pose, ""));
        assert!(error.contains("internal ids"), "{error}");
        assert!(
            !error.contains("Paris"),
            "the refusal quotes no option: {error}"
        );
    }
    assert!(
        session(&t, &id).open_question.is_none(),
        "nothing was posed"
    );
}

#[test]
fn a_numeric_key_or_solution_followed_by_a_unit_is_still_sealed() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let question = recall("Convert 42000 grams to kilograms; give the number.", "42");
    let error = error_of(&call(
        &mut t,
        &id,
        ASK,
        question.clone(),
        "The answer is 42kg.",
    ));
    assert!(error.contains("gives away the answer"), "{error}");
    assert!(
        !call(&mut t, &id, ASK, question, "Convert it.")
            .result
            .is_error
    );

    let practice = t
        .create_session("Practise units", Mode::Practice, &[])
        .unwrap()
        .id;
    let problem = json!({"statement": "Convert 42000 grams to kilograms", "mode": "recall", "solution": "42"});
    t.engine_mut()
        .script(Agent::Practitioner, "", vec![(PROBLEM, problem)]);
    t.engine_mut()
        .script(Agent::Practitioner, "Hint: about 42kg.", vec![]);
    let report = t.advance(&practice).unwrap().remove(0);
    assert_eq!(report.steps[1].text, seal::WITHHELD, "{report:?}");
    assert!(!format!("{report:?}").contains("42kg"), "{report:?}");
}

#[test]
fn a_pose_late_in_a_turn_seals_what_its_earlier_steps_showed() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let read = json!({"concepts": ["cells"]});
    let prober = t.engine_mut();
    prober.script(
        Agent::Prober,
        "The answer is mitochondria.",
        vec![("noesis.learner_read", read)],
    );
    prober.script(
        Agent::Prober,
        "One question:",
        vec![(ASK, recall("Which organelle makes ATP?", SECRET))],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    assert_eq!(report.steps.len(), 2);
    assert_eq!(report.steps[0].text, seal::WITHHELD);
    assert_eq!(report.steps[1].text, "One question:");
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
}

#[test]
fn a_call_id_holding_a_key_committed_later_never_leaves_the_crate() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let read = ToolCall {
        id: format!("toolu_{SECRET}"),
        name: "noesis.learner_read".into(),
        input: json!({"concepts": ["cells"]}),
    };
    let step = Step {
        text: String::new(),
        calls: vec![read],
        usage: Default::default(),
    };
    t.engine_mut().script_step(Agent::Prober, step);
    t.engine_mut().script(
        Agent::Prober,
        "One question:",
        vec![(ASK, recall("Which organelle makes ATP?", SECRET))],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    let public = &report.steps[0].calls[0].id;
    assert!(
        public.starts_with("call_") && !public.contains(SECRET),
        "{public}"
    );
    assert_ne!(public, &report.steps[1].calls[0].id, "ids are per call");
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
    let agent = session(&t, &id).agent;
    let inputs = t.engine().inputs_of(&agent);
    let raw = inputs.iter().any(|input| {
        matches!(input, Input::ToolResults(results)
            if results.iter().any(|r| r.call == format!("toolu_{SECRET}")))
    });
    assert!(raw, "the engine still correlates by its own id");
    assert!(
        !format!("{inputs:?}").contains(SECRET),
        "Debug omits raw ids"
    );
}

#[test]
fn a_key_committed_in_a_later_step_seals_every_earlier_record_of_the_turn() {
    let (_dir, noesis) = fixture();
    add_proposition(&noesis, "p_atp", SECRET, "powerhouse");
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    t.take_events();
    let teacher = t.engine_mut();
    teacher.script(
        Agent::Teacher,
        &format!("Think about the {SECRET}."),
        vec![("noesis.graph_search", json!({"query": "powerhouse"}))],
    );
    teacher.script(Agent::Teacher, "", vec![(PLAN, three_steps())]);
    let mut tagged = recall("Which organelle makes ATP?", SECRET);
    tagged["plan_node"] = json!("a");
    teacher.script(Agent::Teacher, "Now a check:", vec![(ASK, tagged)]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer, "{report:?}");
    assert_eq!(report.steps.len(), 3);
    assert_eq!(report.steps[0].text, seal::WITHHELD);
    let search = &report.steps[0].calls[0];
    assert_eq!(search.result, json!(crate::tools::SEALED), "{search:?}");
    assert_eq!(report.steps[2].text, "Now a check:");
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
    let events = serde_json::to_string(&t.take_events()).unwrap();
    assert!(events.contains("\"revision\""), "a plan event: {events}");
    assert!(!events.contains(SECRET), "{events}");
}

#[test]
fn a_pose_is_refused_when_a_plan_posted_earlier_in_its_turn_shows_its_key() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    t.take_events();
    let leaky = json!({"nodes": [{"id": "a", "title": SECRET}], "edges": []});
    let teacher = t.engine_mut();
    teacher.script(Agent::Teacher, "", vec![(PLAN, leaky)]);
    teacher.script(Agent::Teacher, "", vec![(PLAN, three_steps())]);
    teacher.script(
        Agent::Teacher,
        "",
        vec![(ASK, recall("Which organelle makes ATP?", SECRET))],
    );
    let report = t.advance(&id).unwrap().remove(0);
    let pose = &report.steps[2].calls[0];
    assert!(
        pose.is_error,
        "the superseded plan still goes out: {report:?}"
    );
    assert!(session(&t, &id).open_question.is_none());
}

#[test]
fn a_numeric_key_shown_with_a_unit_before_the_pose_is_sealed() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let teacher = t.engine_mut();
    teacher.script(
        Agent::Teacher,
        "It comes to 42 kg.",
        vec![("noesis.graph_search", json!({"query": "42kg"}))],
    );
    teacher.script(Agent::Teacher, "Roughly 42.", vec![]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(
        report.steps[0].text, "It comes to 42 kg.",
        "nothing sealed yet"
    );

    let teacher = t.engine_mut();
    teacher.script(
        Agent::Teacher,
        "It comes to 42 kg.",
        vec![("noesis.graph_search", json!({"query": "42kg"}))],
    );
    let question = recall("Convert 42000 grams to kilograms; give the number.", "42");
    teacher.script(Agent::Teacher, "Your turn:", vec![(ASK, question)]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer, "{report:?}");
    assert_eq!(report.steps[0].text, seal::WITHHELD);
    assert_eq!(report.steps[0].calls[0].input, json!(crate::tools::SEALED));
    let shown = format!("{report:?}");
    assert!(
        !shown.contains("42 kg") && !shown.contains("42kg"),
        "{shown}"
    );
}

#[test]
fn a_rekeyed_option_value_named_before_the_pose_is_sealed() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    t.take_events();
    let teacher = t.engine_mut();
    teacher.script(
        Agent::Teacher,
        "I will mark opt-right as correct.",
        vec![("noesis.graph_search", json!({"query": "opt-right"}))],
    );
    let pose = choice(
        "Capital of France?",
        &[("opt-right", "Paris"), ("opt-wrong", "Rome")],
        &["opt-right"],
    );
    teacher.script(Agent::Teacher, "Pick one:", vec![(ASK, pose)]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer, "{report:?}");
    assert_eq!(report.steps[0].text, seal::WITHHELD);
    assert_eq!(report.steps[0].calls[0].input, json!(crate::tools::SEALED));
    let events = serde_json::to_string(&t.take_events()).unwrap();
    let view = serde_json::to_string(&session(&t, &id)).unwrap();
    let shown = format!("{report:?}{events}{view}");
    assert!(!shown.contains("opt-right"), "{shown}");
    let mut values = shown_values(&t, &id, &["Paris", "Rome"]);
    values.sort();
    assert_eq!(values, ["1", "2"], "readers see display positions only");
}

#[test]
fn a_choice_prompt_naming_the_correct_label_is_refused() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let pose = choice(
        "The answer is Paris. Which city is the capital of France?",
        &[("opt1", "Paris"), ("opt2", "Rome")],
        &["opt1"],
    );
    let error = error_of(&call(&mut t, &id, ASK, pose, ""));
    assert!(error.contains("gives away"), "{error}");
    assert!(session(&t, &id).open_question.is_none());
}

#[test]
fn a_malformed_sibling_call_does_not_unseal_a_refused_pose_in_its_step() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let leaky = recall("Which organelle, the mitochondria, makes ATP?", SECRET);
    t.engine_mut().script(
        Agent::Teacher,
        "",
        vec![
            (ASK, leaky),
            (ASK, json!(null)),
            ("noesis.graph_search", json!({"query": SECRET})),
        ],
    );
    t.engine_mut().script(Agent::Teacher, "Again.", vec![]);
    let report = t.advance(&id).unwrap().remove(0);
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
    let search = &report.steps[0].calls[2];
    assert_eq!(search.input, json!(crate::tools::SEALED), "{search:?}");
}
