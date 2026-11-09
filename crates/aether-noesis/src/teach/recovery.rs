//! Sealing of raw engine values and recovery from failed deliveries: the
//! read paths and retry paths around the T1–T12 suite.

use serde_json::{Value, json};

use super::faux::FauxEngine;
use super::records::*;
use super::testkit::*;
use super::*;
use crate::prompts::Agent;

const SECRET: &str = "mitochondria";

#[test]
fn teaching_with_an_overlapping_check_is_retried_as_a_lesson() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let lesson = "A rate describes how one quantity changes as another changes. \
        For a straight line, compare the change in height with the change in distance.";
    let check = recall(
        "What describes how one quantity changes as another changes?",
        "rate",
    );
    t.engine_mut()
        .script(Agent::Teacher, lesson, vec![(ASK, check)]);
    t.engine_mut().script(Agent::Teacher, lesson, vec![]);

    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingLearner);
    assert_eq!(report.steps.len(), 1);
    assert_eq!(report.steps[0].text, lesson);
    assert!(report.steps[0].calls.is_empty());
    assert!(session(&t, &id).open_question.is_none());
    assert_eq!(t.engine().reminders, 1);
}

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
fn delivered_answer_keeps_its_turn_pending_across_reload() {
    let (_dir, noesis) = fixture();
    let mut teaching = Teaching::new(&noesis, FauxEngine::new());
    let id = posed_session(&mut teaching);
    assert!(teaching.sessions_pending_turns().unwrap().is_empty());
    answer_choice(&mut teaching, &id, &["Paris"]);
    assert!(db::owed_answers(teaching.conn(), &id).unwrap().is_empty());
    let engine = std::mem::take(teaching.engine_mut());
    drop(teaching);
    let mut reloaded = Teaching::new(&noesis, engine);
    assert_eq!(
        reloaded.sessions_pending_turns().unwrap().as_slice(),
        std::slice::from_ref(&id)
    );
    reloaded.advance(&id).unwrap();
    assert!(reloaded.sessions_pending_turns().unwrap().is_empty());
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
        text: format!("Hint: {SECRET}."),
        calls: vec![ask.clone(), problem],
        ..Step::default()
    };
    let shown = format!("{ask:?} {step:?}");
    for secret in [SECRET, "the-why", "x-equals-4"] {
        assert!(!shown.contains(secret), "{secret} leaked: {shown}");
    }
    assert!(shown.contains("noesis.ask") && !shown.contains("Name it"));
    let parsed = crate::tools::parse(crate::tools::ToolName::Ask, ask.input.clone()).unwrap();
    let search = json!({"query": SECRET});
    let query = crate::tools::parse(crate::tools::ToolName::GraphSearch, search).unwrap();
    let shown = format!("{parsed:?} {query:?}");
    assert!(
        !shown.contains(SECRET) && !shown.contains("Name it"),
        "{shown}"
    );
}

/// A reply that gives its own key away is discarded unseen; the retry,
/// told why, poses the question cleanly.
#[test]
fn a_reply_giving_its_own_key_away_is_discarded_and_made_again() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let leaky = format!("Hint: it is the {SECRET}.");
    let question = recall("Name the powerhouse of the cell", SECRET);
    let prober = t.engine_mut();
    prober.script(Agent::Prober, &leaky, vec![(ASK, question.clone())]);
    prober.script(Agent::Prober, "One question:", vec![(ASK, question)]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    let texts: Vec<&str> = report.steps.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(texts, ["One question:"], "the leaking reply is never kept");
    assert_eq!(t.engine().reminders, 1);
    let open = session(&t, &id)
        .open_question
        .expect("the retry's question");
    assert_eq!(open.prompt, "Name the powerhouse of the cell");
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
}

/// A second leaking reply becomes the neutral step: no question opens and
/// the turn waits for the learner.
#[test]
fn a_second_leaking_reply_becomes_the_neutral_step() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let leaky = format!("Hint: it is the {SECRET}.");
    let question = recall("Name the powerhouse of the cell", SECRET);
    let prober = t.engine_mut();
    for _ in 0..2 {
        prober.script(Agent::Prober, &leaky, vec![(ASK, question.clone())]);
    }
    prober.script(Agent::Prober, "Not reached.", vec![]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingLearner);
    let texts: Vec<&str> = report.steps.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(texts, [NEUTRAL_REPLY]);
    assert!(report.steps[0].calls.is_empty(), "no call of either ran");
    assert!(session(&t, &id).open_question.is_none());
    assert_eq!(t.engine().steps_run, 2, "one retry, no more");
}

/// Each step is kept as it ends, so a key an earlier call of the turn named
/// is on the record before the pose: the pose is refused.
#[test]
fn a_key_an_earlier_call_of_the_turn_named_is_never_posed() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let search = json!({"query": format!("what the {SECRET} does")});
    let prober = t.engine_mut();
    prober.script(Agent::Prober, "", vec![("noesis.graph_search", search)]);
    let question = recall("Name the powerhouse of the cell", SECRET);
    prober.script(Agent::Prober, "One question:", vec![(ASK, question)]);
    prober.script(Agent::Prober, "Then:", vec![(ASK, capital_question())]);
    let report = t.advance(&id).unwrap().remove(0);
    let refused: Vec<bool> = report.steps.iter().map(|s| s.calls[0].is_error).collect();
    assert_eq!(refused, [false, true, false], "{report:?}");
    let open = session(&t, &id).open_question.expect("the fresh question");
    assert_eq!(open.prompt, "Capital of France?");
}

/// The turn's first step is `preface`; its second poses `key`, which that
/// step left on the record, so the pose is refused and the third one runs.
fn pose_after(preface: Step, key: &str) {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let engine = t.engine_mut();
    engine.script_step(Agent::Prober, preface);
    let question = recall("Name the powerhouse of the cell", key);
    engine.script(Agent::Prober, "One question:", vec![(ASK, question)]);
    engine.script(Agent::Prober, "Then:", vec![(ASK, capital_question())]);
    let report = t.advance(&id).unwrap().remove(0);
    let refused: Vec<bool> = report.steps[1..]
        .iter()
        .map(|s| s.calls[0].is_error)
        .collect();
    assert_eq!(refused, [true, false], "{key}: {report:?}");
    let open = session(&t, &id).open_question.expect("the fresh question");
    assert_eq!(open.prompt, "Capital of France?");
}

fn search_step(id: &str, name: &str, input: Value) -> Step {
    Step {
        calls: vec![ToolCall {
            id: id.into(),
            name: name.into(),
            input,
        }],
        ..Step::default()
    }
}

/// The transcript keeps a name no tool has and every literal of an
/// earlier call's input, so a key any of them carries is never posed.
#[test]
fn a_key_an_earlier_calls_name_or_literal_carries_is_never_posed() {
    let search = json!({"query": "cells"});
    pose_after(search_step("call_a", SECRET, search), SECRET);
    let flagged = json!({"query": "cells", "extra": true});
    pose_after(
        search_step("call_a", "noesis.graph_search", flagged),
        "true",
    );
    let nested = json!({"query": "cells", "extra": [null]});
    pose_after(search_step("call_a", "noesis.graph_search", nested), "null");
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
fn a_failed_answer_delivery_is_queued_after_the_commit_and_sent_before_the_next_step() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = posed_session(&mut t);
    let open = session(&t, &id).open_question.expect("posed").id;
    let answer = Answer {
        choices: shown_values(&t, &id, &["Paris"]),
        ..Answer::default()
    };
    t.engine_mut().fail_deliveries = 1;
    let failed = t.answer(&id, &open, &answer);
    assert!(
        matches!(failed, Err(TeachError::Engine(_))),
        "the failed delivery is returned, not dropped"
    );
    let correct = recorded_correctness(&mut t);
    assert_eq!(correct, Some(true), "the record and reveal stand");
    assert!(session(&t, &id).open_question.is_none());
    let agent = session(&t, &id).agent;
    let answers = |t: &Teaching<'_, FauxEngine>| {
        let inputs = t.engine().inputs_of(&agent);
        inputs
            .iter()
            .filter(|i| matches!(i, Input::Answer(_)))
            .count()
    };
    assert_eq!(answers(&t), 0, "the failed delivery reached nothing");
    let retry = t.answer(&id, &open, &answer);
    assert!(
        matches!(retry, Err(TeachError::Conflict(_))),
        "one answer per question"
    );

    let steps_before = t.engine().steps_run;
    t.advance(&id).unwrap();
    assert_eq!(answers(&t), 1, "the queued answer arrived exactly once");
    assert_eq!(t.engine().steps_run, steps_before + 1);
    let inputs = t.engine().inputs_of(&agent);
    assert!(matches!(inputs.last(), Some(Input::Answer(a)) if a["question"] == open.as_str()));
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
        choices: shown_values(&t, &id, &["Rome"]),
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

/// Whatever shape a pose's sealed text takes, a reply whose text shows it
/// is discarded and the retry is kept.
#[test]
fn a_reply_showing_any_shape_of_its_own_sealed_text_is_discarded() {
    let goal = json!({"kind": "goal", "prompt": "?", "answer_key": SECRET});
    let stringified = json!({"kind": "riddle", "prompt": "?", "answer_key": [SECRET]}).to_string();
    let explained = json!({"kind": "recall", "prompt": "Name it", "answer_key": "organelle", "explanation": SECRET});
    for input in [goal, json!(stringified), explained] {
        let (_dir, noesis) = fixture();
        let mut t = Teaching::new(&noesis, FauxEngine::new());
        let id = t
            .create_session("Learn cells", Mode::Teach, &[])
            .unwrap()
            .id;
        let prober = t.engine_mut();
        prober.script(
            Agent::Prober,
            &format!("Hint: {SECRET}."),
            vec![(ASK, input)],
        );
        prober.script(Agent::Prober, "Name it:", vec![(ASK, capital_question())]);
        let report = t.advance(&id).unwrap().remove(0);
        let texts: Vec<&str> = report.steps.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["Name it:"]);
        assert_eq!(t.engine().reminders, 1);
        assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
    }
}

/// A pose whose input is unreadable carries no secret the seal could know,
/// so its reply is kept; the pose is refused, so nothing becomes live.
#[test]
fn an_unreadable_pose_is_refused_and_seals_nothing() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let prober = t.engine_mut();
    prober.script(Agent::Prober, "Hm.", vec![(ASK, json!("not an object"))]);
    prober.script(Agent::Prober, "Then:", vec![(ASK, capital_question())]);
    let report = t.advance(&id).unwrap().remove(0);
    let refused: Vec<bool> = report.steps.iter().map(|s| s.calls[0].is_error).collect();
    assert_eq!(refused, [true, false]);
    assert_eq!(t.engine().reminders, 0);
    assert_eq!(
        session(&t, &id).open_question.expect("open").prompt,
        "Capital of France?"
    );
}

#[test]
fn a_tagged_probe_is_refused_then_corrected_within_the_turn() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let clean = recall("Which organelle makes ATP?", SECRET);
    let mut tagged = clean.clone();
    tagged["plan_node"] = json!("cell");
    t.engine_mut()
        .script(Agent::Prober, "", vec![(ASK, tagged)]);
    t.engine_mut().script(Agent::Prober, "", vec![(ASK, clean)]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    assert_eq!(report.steps.len(), 2);
    let refused = &report.steps[0].calls[0];
    assert!(refused.is_error);
    assert!(
        refused.result["error"]
            .as_str()
            .unwrap()
            .contains("omit `plan_node`")
    );
    assert!(!report.steps[1].calls[0].is_error);
    assert_eq!(session(&t, &id).open_question.unwrap().plan_node, None);
    assert!(!format!("{report:?}").contains(SECRET));
    let agent = session(&t, &id).agent;
    let inputs = t.engine().inputs_of(&agent);
    assert!(inputs.iter().any(|input| matches!(input, Input::ToolResults(results)
        if results.iter().any(|result| result.is_error && result.content["error"].as_str().unwrap().contains("omit `plan_node`")))));
}

#[test]
fn repeated_tagged_probes_stop_at_the_existing_step_limit() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let mut tagged = capital_question();
    tagged["plan_node"] = json!("cell");
    for _ in 0..service::MAX_STEPS {
        t.engine_mut()
            .script(Agent::Prober, "", vec![(ASK, tagged.clone())]);
    }
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::StepLimit);
    assert_eq!(report.steps.len(), service::MAX_STEPS);
    assert!(report.steps.iter().all(|step| step.calls[0].is_error));
    assert!(session(&t, &id).open_question.is_none());
}

#[test]
fn a_teacher_goal_question_must_omit_the_plan_node() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let mut goal =
        json!({"kind": "goal", "prompt": "What would you like to achieve?", "plan_node": "a"});
    t.engine_mut()
        .script(Agent::Teacher, "", vec![(ASK, goal.clone())]);
    goal.as_object_mut().unwrap().remove("plan_node");
    t.engine_mut().script(Agent::Teacher, "", vec![(ASK, goal)]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    assert_eq!(report.steps.len(), 2);
    let refused = &report.steps[0].calls[0];
    assert!(refused.is_error);
    assert!(
        refused.result["error"]
            .as_str()
            .unwrap()
            .contains("omit `plan_node`")
    );
    assert!(!report.steps[1].calls[0].is_error);
    assert_eq!(session(&t, &id).open_question.unwrap().plan_node, None);
}

/// A reply that shows its own problem's solution is discarded; the retry
/// poses the problem, and a second problem waits until it is concluded.
#[test]
fn a_reply_showing_its_own_solution_is_discarded_and_the_retry_poses() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise algebra", Mode::Practice, &[])
        .unwrap()
        .id;
    let problem =
        json!({"statement": "Solve 2x = 8", "mode": "recall", "solution": "x = 4 by halving"});
    let practitioner = t.engine_mut();
    practitioner.script(
        Agent::Practitioner,
        "Try this: x = 4 by halving",
        vec![(PROBLEM, problem.clone())],
    );
    practitioner.script(Agent::Practitioner, "Try this.", vec![(PROBLEM, problem)]);
    let other =
        json!({"statement": "Solve 3y = 9", "mode": "recall", "solution": "y = 3 by thirds"});
    practitioner.script(Agent::Practitioner, "Or this.", vec![(PROBLEM, other)]);
    let report = t.advance(&id).unwrap().remove(0);
    let texts: Vec<&str> = report.steps.iter().map(|s| s.text.as_str()).collect();
    // A problem does not end the turn; the unscripted step after it is empty.
    assert_eq!(texts, ["Try this.", "Or this.", ""]);
    let refused: Vec<bool> = report.steps[..2]
        .iter()
        .map(|s| s.calls[0].is_error)
        .collect();
    assert_eq!(refused, [false, true], "one problem is active at a time");
    assert!(!format!("{report:?}").contains("halving"));
}

/// A step that runs while a solution is unrevealed is checked against it,
/// so the engine is handed the solution as the live seal.
#[test]
fn a_step_while_a_problem_is_active_is_sealed_against_its_solution() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise algebra", Mode::Practice, &[])
        .unwrap()
        .id;
    let solution = "x = 4 by halving";
    let problem = json!({"statement": "Solve 2x = 8", "mode": "recall", "solution": solution});
    let practitioner = t.engine_mut();
    practitioner.script(Agent::Practitioner, "Here it is.", vec![(PROBLEM, problem)]);
    practitioner.script(Agent::Practitioner, "Take your time.", vec![]);
    t.advance(&id).unwrap();
    let seals = &t.engine().step_seals;
    assert_eq!(seals.len(), 2, "one seal per step");
    assert!(seals[0].is_empty(), "nothing is sealed before the problem");
    assert!(
        seals[1].holds(solution),
        "the active solution seals the next step"
    );
}

#[test]
fn a_later_step_cannot_show_the_active_solution_or_a_recall_key_dressed_as_an_option() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise algebra", Mode::Practice, &[])
        .unwrap()
        .id;
    let solution = "x = 4 by halving";
    let problem = json!({"statement": "Solve 2x = 8", "mode": "recall", "solution": solution});
    let conclude = json!({"outcome": "solved", "retest": false});
    let practitioner = t.engine_mut();
    practitioner.script(Agent::Practitioner, "Here it is.", vec![(PROBLEM, problem)]);
    practitioner.script(Agent::Practitioner, solution, vec![]);
    let report = t.advance(&id).unwrap().remove(0);
    let texts: Vec<&str> = report.steps.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(
        texts,
        ["Here it is.", ""],
        "the reply showing it was discarded"
    );
    t.engine_mut()
        .script(Agent::Practitioner, solution, vec![(CONCLUDE, conclude)]);
    let after = t.advance(&id).unwrap().remove(0);
    assert_eq!(
        after.steps[0].text, solution,
        "conclude reveals the solution"
    );

    let dressed = json!({"kind": "recall", "prompt": "Name the organelle", "answer_key": SECRET,
        "options": [{"value": SECRET, "label": "organelle"}]});
    let texts = crate::tools::sealed_texts(ASK, &dressed).unwrap();
    assert_eq!(texts, [SECRET], "a recall key is never swapped for a label");
}

#[test]
fn a_tool_input_repeating_the_active_solution_is_discarded() {
    let (_dir, noesis) = fixture();
    add_proposition(&noesis, "p_cell", "the organelle", "powerhouse");
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise cells", Mode::Practice, &[])
        .unwrap()
        .id;
    let problem =
        json!({"statement": "Name the powerhouse", "mode": "recall", "solution": "the organelle"});
    let search = json!({"query": "the organelle"});
    let practitioner = t.engine_mut();
    practitioner.script(Agent::Practitioner, "", vec![(PROBLEM, problem)]);
    practitioner.script(
        Agent::Practitioner,
        "",
        vec![("noesis.graph_search", search)],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(t.engine().reminders, 1, "{report:?}");
    assert!(
        !format!("{report:?}").contains("the organelle"),
        "{report:?}"
    );
    let agent = session(&t, &id).agent;
    let inputs = format!("{:?}", t.engine().inputs_of(&agent));
    assert!(!inputs.contains("the organelle"), "{inputs}");
}

#[test]
fn visible_fields_that_repeat_a_sealed_value_are_refused_before_anything_is_stored() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let mut tagged = recall("Name the organelle", SECRET);
    tagged["plan_node"] = json!(SECRET);
    let mut tagged_concept = recall("Name the organelle", SECRET);
    tagged_concept["concepts"] = json!([SECRET]);
    let in_prompt = recall(&format!("Name it. Answer: {SECRET}"), SECRET);
    for input in [tagged, tagged_concept, in_prompt] {
        let error = error_of(&call(&mut t, &id, ASK, input, ""));
        assert!(!error.contains(SECRET), "{error}");
        assert!(error.contains("gives away"), "{error}");
    }
    assert!(session(&t, &id).open_question.is_none());

    let practice = t
        .create_session("Practise algebra", Mode::Practice, &[])
        .unwrap()
        .id;
    t.take_events();
    let told = json!({"statement": "Solve 2x = 8. Answer: x = 4 by halving.", "mode": "recall",
        "solution": "x = 4 by halving"});
    let error = error_of(&call(&mut t, &practice, PROBLEM, told, ""));
    assert!(error.contains("gives away the solution"), "{error}");
    assert!(session(&t, &practice).active_problem.is_none());
    assert!(t.take_events().is_empty(), "nothing was posed");
}

#[test]
fn unknown_field_names_and_option_values_cannot_carry_a_secret() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise cells", Mode::Practice, &[])
        .unwrap()
        .id;
    let problem = json!({"statement": "Name the powerhouse", "mode": "recall", "solution": SECRET});
    let search = json!({"query": "cell", SECRET: true});
    let practitioner = t.engine_mut();
    practitioner.script(Agent::Practitioner, "", vec![(PROBLEM, problem)]);
    practitioner.script(
        Agent::Practitioner,
        "",
        vec![("noesis.graph_search", search)],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(t.engine().reminders, 1, "a key naming it is read too");
    assert!(report.steps[1].calls.is_empty(), "{report:?}");
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
    let agent = session(&t, &id).agent;
    assert!(!format!("{:?}", t.engine().inputs_of(&agent)).contains(SECRET));

    let teach = teacher_session(&mut t);
    let why = "because gravity pulls downward";
    let mut hidden = choice("Which way?", &[("a", "Down"), (why, "Up")], &["a"]);
    hidden["explanation"] = json!(why);
    assert!(error_of(&call(&mut t, &teach, ASK, hidden, "")).contains("gives away"));
}

#[test]
fn a_numeric_argument_cannot_carry_a_numeric_solution() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise sums", Mode::Practice, &[])
        .unwrap()
        .id;
    let problem = json!({"statement": "What is six times seven?", "mode": "recall", "solution": "9876543210"});
    let practitioner = t.engine_mut();
    practitioner.script(Agent::Practitioner, "", vec![(PROBLEM, problem)]);
    practitioner.script(
        Agent::Practitioner,
        "",
        vec![("noesis.graph_search", json!({"query": 9876543210_u64}))],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(t.engine().reminders, 1, "{report:?}");
    assert!(report.steps[1].calls.is_empty(), "{report:?}");
    assert!(!format!("{report:?}").contains("9876543210"), "{report:?}");
}

#[test]
fn a_plan_whose_ids_hold_the_key_its_reply_poses_is_discarded() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let twice = json!({"nodes": [{"id": SECRET, "title": "One"}, {"id": SECRET, "title": "Two"}],
        "edges": []});
    t.engine_mut().script(
        Agent::Teacher,
        "",
        vec![
            (PLAN, twice),
            (ASK, recall("Which organelle makes ATP?", SECRET)),
        ],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingLearner, "nothing was posed");
    assert_eq!(t.engine().reminders, 1, "{report:?}");
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
    let agent = session(&t, &id).agent;
    assert!(!format!("{:?}", t.engine().inputs_of(&agent)).contains(SECRET));
}

#[test]
fn a_null_argument_cannot_carry_a_null_solution() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise JSON", Mode::Practice, &[])
        .unwrap()
        .id;
    let problem = json!({"statement": "Which JSON literal stands for an absent value?",
        "mode": "recall", "solution": "null"});
    let practitioner = t.engine_mut();
    practitioner.script(Agent::Practitioner, "", vec![(PROBLEM, problem)]);
    practitioner.script(
        Agent::Practitioner,
        "",
        vec![("noesis.learner_read", json!({"query": null}))],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(t.engine().reminders, 1, "a null is read as its text");
    assert!(report.steps[1].calls.is_empty(), "{report:?}");
    assert!(!format!("{report:?}").contains("null"), "{report:?}");
}

#[test]
fn an_overlapping_or_marker_like_solution_is_still_matched() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise grammar", Mode::Practice, &[])
        .unwrap()
        .id;
    let doubled = json!({"statement": "Which two words repeat in the past perfect of have?",
        "mode": "recall", "solution": "had had"});
    let practitioner = t.engine_mut();
    practitioner.script(Agent::Practitioner, "", vec![(PROBLEM, doubled)]);
    practitioner.script(Agent::Practitioner, "Chad had had enough.", vec![]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(t.engine().reminders, 1, "{report:?}");
    assert!(!format!("{report:?}").contains("had had"), "{report:?}");

    let marked = t
        .create_session("Practise words", Mode::Practice, &[])
        .unwrap()
        .id;
    let problem = json!({"statement": "Which word means closed tight?", "mode": "recall",
        "solution": "sealed"});
    let search = json!({"query": "vocabulary", "sealed": true});
    let practitioner = t.engine_mut();
    practitioner.script(Agent::Practitioner, "", vec![(PROBLEM, problem)]);
    practitioner.script(
        Agent::Practitioner,
        "",
        vec![("noesis.graph_search", search)],
    );
    let report = t.advance(&marked).unwrap().remove(0);
    assert_eq!(t.engine().reminders, 2, "a key named like the marker");
    assert!(report.steps[1].calls.is_empty(), "{report:?}");

    let authored = t
        .create_session("Practise tenses", Mode::Practice, &[])
        .unwrap()
        .id;
    let told = json!({"statement": "Chad had had enough. Which two words repeat?",
        "mode": "recall", "solution": "had had"});
    let error = error_of(&call(&mut t, &authored, PROBLEM, told, ""));
    assert!(error.contains("gives away the solution"), "{error}");
}

#[test]
fn a_target_whose_proposition_id_repeats_the_solution_is_refused() {
    let (_dir, noesis) = fixture();
    add_proposition(&noesis, "p_mitochondria", "the organelle", "powerhouse");
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise cells", Mode::Practice, &[])
        .unwrap()
        .id;
    t.take_events();
    let problem = json!({"statement": "Name the organelle that supplies ATP", "mode": "recall",
        "targets": ["powerhouse"], "solution": SECRET});
    let error = error_of(&call(&mut t, &id, PROBLEM, problem, ""));
    assert!(error.contains("gives away the solution"), "{error}");
    assert!(!error.contains(SECRET), "{error}");
    let view = session(&t, &id);
    assert!(view.active_problem.is_none());
    assert!(!format!("{view:?}").contains(SECRET), "{view:?}");
    assert!(t.take_events().is_empty(), "nothing was posed");
}

#[test]
fn the_record_of_a_choice_never_marks_which_options_are_correct() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let options = [("opt1", "Paris"), ("opt2", "Rome"), ("opt3", "Madrid")];
    let single = choice("Which city is the capital of France?", &options, &["opt1"]);
    t.engine_mut()
        .script(Agent::Teacher, "", vec![(ASK, single.clone())]);
    let first = t.advance(&id).unwrap().remove(0);
    assert_eq!(first.steps[0].calls[0].input, json!(crate::tools::SEALED));
    answer_choice(&mut t, &id, &["Paris"]);

    let mut multi = choice("Which cities are capitals?", &options, &["opt1", "opt3"]);
    multi["multi"] = json!(true);
    t.engine_mut()
        .script(Agent::Teacher, "", vec![(ASK, multi)]);
    let second = t.advance(&id).unwrap().remove(0);
    assert_eq!(second.end, TurnEnd::AwaitingAnswer, "{second:?}");
    assert_eq!(second.steps[0].calls[0].input, json!(crate::tools::SEALED));
    assert!(!format!("{second:?}").contains("Paris"), "{second:?}");
}

/// The correctness the last `noesis.answer_recorded` event revealed.
fn recorded_correctness(t: &mut Teaching<'_, FauxEngine>) -> Option<bool> {
    let events = t.take_events();
    let result = events.into_iter().rev().find_map(|event| match event {
        crate::teach::records::NoesisEvent::AnswerRecorded { result, .. } => Some(result),
        _ => None,
    });
    result.expect("the answer is announced").correct
}
