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
fn a_stringified_argument_escaping_the_active_solution_is_discarded() {
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
    assert_eq!(t.engine().reminders, 1, "{report:?}");
    assert!(report.steps[1].calls.is_empty(), "the retry replaced it");
    let shown = format!("{report:?}");
    assert!(!shown.contains("u03c0") && !shown.contains('π'), "{shown}");
}

#[test]
fn a_reply_whose_plan_shows_its_own_key_is_discarded_in_either_order() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    t.take_events();
    let leaky_plan = json!({"nodes": [{"id": "cell", "title": SECRET}], "edges": []});
    let mut tagged = recall("Which organelle makes ATP?", SECRET);
    tagged["plan_node"] = json!("cell");
    let orders = [
        vec![(PLAN, leaky_plan.clone()), (ASK, tagged.clone())],
        vec![(ASK, tagged), (PLAN, leaky_plan)],
    ];
    for (turn, calls) in orders.into_iter().enumerate() {
        t.engine_mut().script(Agent::Teacher, "", calls);
        t.engine_mut()
            .script(Agent::Teacher, "Let us rethink.", vec![]);
        let report = t.advance(&id).unwrap().remove(0);
        assert_eq!(t.engine().reminders, turn + 1, "{report:?}");
        assert_eq!(report.steps[0].text, "Let us rethink.");
        assert!(report.steps[0].calls.is_empty(), "{report:?}");
        assert!(session(&t, &id).open_question.is_none());
    }
    let events = serde_json::to_string(&t.take_events()).unwrap();
    assert!(!events.contains(SECRET), "the plan never ran: {events}");
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
    let answer = Answer {
        choices: shown_values(&t, &id, &["Paris"]),
        ..Answer::default()
    };
    let failed = t.answer(&id, &open, &answer);
    assert!(matches!(failed, Err(TeachError::Engine(_))), "{failed:?}");
    assert!(session(&t, &id).open_question.is_none(), "still recorded");

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
    assert_eq!(t.engine().reminders, 1, "{report:?}");
    assert_eq!(report.steps[0].text, "Rethinking.", "{report:?}");
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
    assert_eq!(t.engine().reminders, 1, "{report:?}");
    assert_eq!(report.steps[1].text, "", "the retry replaced it");
    assert!(!format!("{report:?}").contains("42kg"), "{report:?}");
}

#[test]
fn a_pose_whose_key_an_earlier_step_showed_is_refused() {
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
    assert_eq!(report.end, TurnEnd::AwaitingLearner, "{report:?}");
    assert_eq!(report.steps[0].text, "The answer is mitochondria.");
    assert_eq!(report.steps[1].text, "One question:");
    assert!(report.steps[1].calls[0].is_error, "{report:?}");
    assert_eq!(t.engine().reminders, 0, "a refused pose is no leak");
    assert!(session(&t, &id).open_question.is_none());
}

#[test]
fn a_search_result_showing_a_key_posed_later_in_the_turn_is_sealed() {
    let (_dir, noesis) = fixture();
    add_proposition(&noesis, "p_atp", SECRET, "powerhouse");
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    t.take_events();
    let teacher = t.engine_mut();
    teacher.script(
        Agent::Teacher,
        "Let me look.",
        vec![("noesis.graph_search", json!({"query": "powerhouse"}))],
    );
    teacher.script(Agent::Teacher, "", vec![(PLAN, three_steps())]);
    let mut tagged = recall("Which organelle makes ATP?", SECRET);
    tagged["plan_node"] = json!("a");
    teacher.script(Agent::Teacher, "Now a check:", vec![(ASK, tagged)]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer, "{report:?}");
    let search = &report.steps[0].calls[0];
    assert_eq!(search.result, json!(crate::tools::SEALED), "{search:?}");
    assert!(!report.steps[2].calls[0].is_error, "{report:?}");
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
fn a_numeric_key_shown_with_a_unit_before_the_pose_is_refused() {
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
    assert_eq!(report.end, TurnEnd::AwaitingLearner, "{report:?}");
    assert!(report.steps[1].calls[0].is_error, "{report:?}");
    assert_eq!(t.engine().reminders, 0, "a refused pose is no leak");
    assert!(session(&t, &id).open_question.is_none());
}

#[test]
fn a_rekeyed_option_value_named_before_the_pose_never_reaches_the_question() {
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
    let question = serde_json::to_string(&session(&t, &id).open_question).unwrap();
    assert!(question.contains("Paris"), "{question}");
    assert!(!question.contains("opt-right"), "{question}");
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
fn a_malformed_sibling_call_does_not_hide_its_reply_s_leak() {
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
    assert_eq!(t.engine().reminders, 1, "{report:?}");
    assert_eq!(report.steps[0].text, "Again.");
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
}

#[test]
fn a_key_or_solution_ending_in_punctuation_is_matched_without_it() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let question = recall("Which city is the capital of France?", "Paris.");
    let error = error_of(&call(&mut t, &id, ASK, question.clone(), "It is Paris"));
    assert!(error.contains("gives away the answer"), "{error}");
    assert!(!call(&mut t, &id, ASK, question, "Name it.").result.is_error);

    let practice = t
        .create_session("Practise algebra", Mode::Practice, &[])
        .unwrap()
        .id;
    let problem = json!({"statement": "Solve 2x = 8", "mode": "recall", "solution": "x = 4."});
    t.engine_mut().script(
        Agent::Practitioner,
        "Try this; answer is x = 4",
        vec![(PROBLEM, problem)],
    );
    let report = t.advance(&practice).unwrap().remove(0);
    assert_eq!(t.engine().reminders, 1, "{report:?}");
    assert_eq!(report.steps[0].text, "", "the retry replaced it");
    assert!(!format!("{report:?}").contains("x = 4"), "{report:?}");
}

#[test]
fn answered_recall_delivers_the_expected_answer_after_a_failed_delivery() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![(ASK, recall("Name the organelle", "mitochondria"))],
    );
    t.advance(&id).unwrap();
    let open = session(&t, &id).open_question.unwrap().id;
    t.engine_mut().fail_deliveries = 1;
    let answer = Answer {
        text: Some("mitochondria".into()),
        ..Answer::default()
    };
    assert!(t.answer(&id, &open, &answer).is_err());
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![(
            "noesis.judge_recall",
            json!({"question": open, "correct": true}),
        )],
    );
    t.engine_mut().script(Agent::Prober, "Correct.", vec![]);
    t.advance(&id).unwrap();
    let agent = session(&t, &id).agent;
    let inputs = t.engine().inputs_of(&agent);
    let answers: Vec<_> = inputs
        .iter()
        .filter_map(|input| match input {
            Input::Answer(answer) => Some(answer),
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["answer_key"], "mitochondria");
    assert_eq!(answers[0]["text"], "mitochondria");
}

fn judge_recall_for_test(
    t: &mut Teaching<'_, FauxEngine>,
    id: &str,
    question: &str,
    correct: bool,
) -> bool {
    let args = json!({"question": question, "correct": correct, "misconception": "Confuses cells and organs"});
    call(t, id, "noesis.judge_recall", args, "").result.is_error
}

#[test]
fn recall_judgment_refuses_unanswered_foreign_and_duplicate_questions() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![(ASK, recall("Name the organelle", SECRET))],
    );
    t.advance(&id).unwrap();
    let question = session(&t, &id).open_question.unwrap().id;
    assert!(judge_recall_for_test(&mut t, &id, &question, false));
    t.answer(
        &id,
        &question,
        &Answer {
            text: Some("nucleus".into()),
            confidence: Some(90),
            ..Answer::default()
        },
    )
    .unwrap();
    assert!(judge_recall_for_test(&mut t, &id, "q_other", false));
    assert!(!judge_recall_for_test(&mut t, &id, &question, false));
    assert!(judge_recall_for_test(&mut t, &id, &question, true));
    let result = session(&t, &id).questions.pop().unwrap().result.unwrap();
    assert_eq!(result.correct, Some(false));
    assert_eq!(
        result.misconception.as_deref(),
        Some("Confuses cells and organs")
    );
    let evidence = t.take_evidence();
    assert_eq!(evidence.len(), 2);
    assert_eq!(evidence[0].outcome, EvidenceOutcome::Incorrect);
    assert_eq!(evidence[1].kind, EvidenceKind::MisconceptionObserved);
}

#[test]
fn recall_judgment_refuses_wrong_agent_and_dont_know() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![(ASK, recall("Name the organelle", SECRET))],
    );
    t.advance(&id).unwrap();
    let question = session(&t, &id).open_question.unwrap().id;
    let tool = ToolCall {
        id: "judge".into(),
        name: "noesis.judge_recall".into(),
        input: json!({"question": question, "correct": true}),
    };
    let other = Caller {
        agent: "agt_other".into(),
        role: "noesis.prober".into(),
    };
    assert!(t.call_tool(&id, &other, &tool, "").unwrap().result.is_error);
    t.answer(
        &id,
        &question,
        &Answer {
            dont_know: true,
            ..Answer::default()
        },
    )
    .unwrap();
    assert!(
        call(&mut t, &id, "noesis.judge_recall", tool.input, "")
            .result
            .is_error
    );
    let evidence = t.take_evidence();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].outcome, EvidenceOutcome::DontKnow);
}
