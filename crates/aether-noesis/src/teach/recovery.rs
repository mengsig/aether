//! Sealing of raw engine values and recovery from failed deliveries: the
//! read paths and retry paths around the T1–T12 suite.

use serde_json::json;

use super::faux::FauxEngine;
use super::records::*;
use super::testkit::*;
use super::*;
use crate::prompts::Agent;

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
        text: format!("Hint: {SECRET}."),
        calls: vec![ask.clone(), problem],
        usage: Default::default(),
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
fn a_failed_answer_delivery_is_queued_after_the_commit_and_sent_before_the_next_step() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = posed_session(&mut t);
    let open = session(&t, &id).open_question.expect("posed").id;
    let answer = Answer {
        choices: vec!["a".into()],
        ..Answer::default()
    };
    t.engine_mut().fail_deliveries = 1;
    let result = t.answer(&id, &open, &answer).unwrap();
    assert_eq!(result.correct, Some(true), "the record and reveal stand");
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

#[test]
fn step_text_is_withheld_for_every_sealed_text_shape() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn cells", Mode::Teach, &[])
        .unwrap()
        .id;
    let leaky = format!("Hint: {SECRET}.");
    let goal = json!({"kind": "goal", "prompt": "?", "answer_key": SECRET});
    let stringified = json!({"kind": "riddle", "prompt": "?", "answer_key": [SECRET]}).to_string();
    let explained = json!({"kind": "recall", "prompt": "Name it", "answer_key": "organelle", "explanation": SECRET});
    let unreadable = json!("not an object");
    let prober = t.engine_mut();
    // The refused shapes come first; the explained pose succeeds and parks.
    for input in [goal, json!(stringified), unreadable, explained] {
        prober.script(Agent::Prober, &leaky, vec![(ASK, input)]);
    }
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    let texts: Vec<&str> = report.steps.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(texts, [WITHHELD; 4]);
    let refused: Vec<bool> = report.steps.iter().map(|s| s.calls[0].is_error).collect();
    assert_eq!(refused, [true, true, true, false]);
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
}

#[test]
fn a_practice_solution_in_the_step_text_is_withheld_until_conclude() {
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
        vec![(PROBLEM, problem)],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.steps[0].text, WITHHELD);
    assert!(
        !report.steps[0].calls[0].is_error,
        "the problem itself is posed"
    );
    assert!(!format!("{report:?}").contains("halving"));
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
    assert_eq!(texts, ["Here it is.", WITHHELD]);
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
fn tool_inputs_and_results_never_repeat_an_active_solution() {
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
fn unknown_field_names_and_option_values_cannot_carry_a_sealed_value() {
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
    assert!(
        report.steps[1].calls[0].is_error,
        "an unknown field is refused"
    );
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
    assert_eq!(report.steps[1].calls[0].input, json!(crate::tools::SEALED));
    assert!(!format!("{report:?}").contains("9876543210"), "{report:?}");
}

#[test]
fn a_refusal_in_the_batch_that_poses_a_question_cannot_quote_its_key() {
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
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    let refused = &report.steps[0].calls[0];
    assert!(refused.is_error, "the duplicate ids are refused");
    assert_eq!(refused.result, json!({"error": seal::REFUSAL_WITHHELD}));
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
    assert_eq!(report.steps[1].calls[0].input, json!(crate::tools::SEALED));
    assert!(!format!("{report:?}").contains("null"), "{report:?}");
}

#[test]
fn an_overlapping_or_marker_like_solution_is_still_sealed() {
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
    assert_eq!(report.steps[1].text, WITHHELD);
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
    assert!(
        report.steps[1].calls[0].is_error,
        "an unknown field is refused"
    );

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
    let options = [("a", "Paris"), ("b", "Rome"), ("c", "Madrid")];
    let single = choice("Which city is a capital in Europe?", &options, &["a"]);
    t.engine_mut()
        .script(Agent::Teacher, "", vec![(ASK, single.clone())]);
    let first = t.advance(&id).unwrap().remove(0);
    assert_eq!(first.steps[0].calls[0].input, json!(crate::tools::SEALED));
    answer_choice(&mut t, &id, &["a"]);

    let mut multi = choice("Which cities are capitals?", &options, &["a", "c"]);
    multi["multi"] = json!(true);
    t.engine_mut()
        .script(Agent::Teacher, "", vec![(ASK, multi), (ASK, single)]);
    let second = t.advance(&id).unwrap().remove(0);
    for call in &second.steps[0].calls {
        assert_eq!(call.input, json!(crate::tools::SEALED), "{call:?}");
    }
    assert!(!format!("{second:?}").contains("Paris"), "{second:?}");
}
