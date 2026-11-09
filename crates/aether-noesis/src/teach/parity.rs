//! The teaching parity suite, T1–T12 of NOESIS.md §6, against the scripted
//! faux engine. No test reaches a model.

use serde_json::{Value, json};

use super::faux::FauxEngine;
use super::phase::fold;
use super::question::{rekey, shuffle};
use super::records::*;
use super::testkit::*;
use super::*;
use crate::budget::{LaneSpend, Refusal};
use crate::prompts::Agent;
use crate::settings::NoesisSettings;
use crate::tools::{self, SEALED};

#[test]
fn t01_practice_gets_the_practitioner_and_the_teacher_only_follows_a_successful_probe_done() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let practice = t
        .create_session("Practise derivatives", Mode::Practice, &[])
        .unwrap();
    assert_eq!(kind_of(&t, &practice.agent), Agent::Practitioner);
    assert_eq!(practice.phase, Phase::Practice);
    let teach = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap();
    assert_eq!(kind_of(&t, &teach.agent), Agent::Prober);
    assert_eq!(teach.phase, Phase::Probe);

    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![(PROBE_DONE, json!({"strands": []}))],
    );
    let reports = t.advance(&teach.id).unwrap();
    assert!(
        reports[0].steps[0].calls[0].is_error,
        "an empty assessment is refused"
    );
    let teachers = |t: &Teaching<'_, FauxEngine>| {
        t.engine()
            .spawned
            .iter()
            .filter(|(_, spec)| spec.agent == Agent::Teacher)
            .count()
    };
    assert_eq!(teachers(&t), 0, "a refused probe_done starts no Teacher");
    assert_eq!(session(&t, &teach.id).phase, Phase::Probe);

    t.engine_mut()
        .script(Agent::Prober, "", vec![(PROBE_DONE, strands())]);
    let reports = t.advance(&teach.id).unwrap();
    assert_eq!(reports[0].end, TurnEnd::HandedOff);
    assert_eq!(teachers(&t), 1);
    let view = session(&t, &teach.id);
    assert_eq!(kind_of(&t, &view.agent), Agent::Teacher);
    assert_eq!(view.phase, Phase::Plan);
}

#[test]
fn t02_probe_done_is_refused_while_a_question_is_open_and_on_a_second_call() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut().script(
        Agent::Prober,
        "First:",
        vec![(ASK, capital_question()), (PROBE_DONE, strands())],
    );
    let reports = t.advance(&id).unwrap();
    let calls = &reports[0].steps[0].calls;
    assert!(!calls[0].is_error);
    assert!(calls[1].is_error);
    assert!(
        calls[1].result["error"]
            .as_str()
            .unwrap()
            .contains("question is open")
    );
    assert_eq!(session(&t, &id).phase, Phase::Probe);
    answer_choice(&mut t, &id, &["Paris"]);

    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![(PROBE_DONE, strands()), (PROBE_DONE, strands())],
    );
    t.engine_mut()
        .fail_next(Agent::Teacher, EngineError::NotSent("wake failed".into()));
    let error = t.advance(&id).unwrap_err();
    assert!(
        matches!(error, TeachError::Engine(EngineError::NotSent(_))),
        "{error}"
    );
    let prober = t.engine().spawned[0].0.clone();
    let view = session(&t, &id);
    assert_eq!(
        view.phase,
        Phase::Plan,
        "the hand-off is committed before the wake"
    );
    assert_ne!(view.agent, prober);

    // The failed wake left the hand-off persisted: running the turn recovers.
    t.engine_mut()
        .script(Agent::Teacher, "Plan:", vec![(PLAN, three_steps())]);
    let report = t.run_turn(&id).unwrap();
    assert!(!report.steps[0].calls[0].is_error);
    let handoff = t.engine().inputs_of(&view.agent);
    assert!(matches!(handoff[0], Input::Handoff(text) if text.contains("epsilon-delta")));
    assert_eq!(
        handoff.len(),
        3,
        "the hand-off, plan result and explanation"
    );
    assert!(matches!(handoff[2], Input::StandaloneText(text) if text == "Plan:"));
}

#[test]
fn t02_the_acknowledgement_is_fixed_and_a_second_call_in_the_batch_is_refused() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![(PROBE_DONE, strands()), (PROBE_DONE, strands())],
    );
    let reports = t.advance(&id).unwrap();
    let calls = &reports[0].steps[0].calls;
    assert_eq!(calls[0].result, json!(PROBE_DONE_ACK));
    assert!(
        calls[1].result["error"]
            .as_str()
            .unwrap()
            .contains("already recorded")
    );
    assert_eq!(
        reports.len(),
        2,
        "the hand-off runs the Teacher's first turn"
    );
    let observed: Vec<EvidenceDraft> = t.take_evidence();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].kind, EvidenceKind::MisconceptionObserved);
    assert!(!observed[0].supports_mastery());
}

fn assert_phase_folds(
    t: &mut Teaching<'_, FauxEngine>,
    events: &mut Vec<NoesisEvent>,
    id: &str,
    expected: Phase,
) {
    events.extend(t.take_events());
    let stored = session(t, id).phase;
    assert_eq!(stored, expected);
    assert_eq!(
        fold(id, events),
        Some(stored),
        "the stored phase equals the fold"
    );
}

#[test]
fn t03_probe_to_plan_on_probe_done_and_plan_to_teach_on_the_first_tagged_question() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let mut events = Vec::new();
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    assert_phase_folds(&mut t, &mut events, &id, Phase::Probe);
    t.engine_mut()
        .script(Agent::Prober, "", vec![(PROBE_DONE, strands())]);
    t.engine_mut()
        .script(Agent::Teacher, "Plan:", vec![(PLAN, three_steps())]);
    t.advance(&id).unwrap();
    assert_phase_folds(&mut t, &mut events, &id, Phase::Plan);

    let confirm = json!({"kind": "goal", "prompt": "Does this plan suit you?", "options": ["yes", "change it"]});
    assert!(!call(&mut t, &id, ASK, confirm, "").result.is_error);
    assert_phase_folds(&mut t, &mut events, &id, Phase::Plan);
    answer_choice(&mut t, &id, &["yes"]);
    let rewrite = json!({"nodes": [{"id": "a", "title": "A"}], "edges": []});
    assert!(!call(&mut t, &id, PLAN, rewrite, "").result.is_error);
    assert_phase_folds(&mut t, &mut events, &id, Phase::Plan);

    let mut check = capital_question();
    check["plan_node"] = json!("a");
    assert!(!call(&mut t, &id, ASK, check, "").result.is_error);
    assert_phase_folds(&mut t, &mut events, &id, Phase::Teach);
    let changes: Vec<(Phase, Phase)> = events
        .iter()
        .filter_map(|event| match event {
            NoesisEvent::PhaseChanged { from, to, .. } => Some((*from, *to)),
            _ => None,
        })
        .collect();
    assert_eq!(
        changes,
        vec![(Phase::Probe, Phase::Plan), (Phase::Plan, Phase::Teach)]
    );
}

#[test]
fn t04_a_pose_ends_the_run_even_in_a_mixed_batch_and_the_pending_question_is_derived() {
    let (_dir, noesis) = fixture();
    let mut engine = FauxEngine::new();
    engine.script(
        Agent::Prober,
        "Let me check one thing.",
        vec![
            ("noesis.graph_search", json!({"query": "limit"})),
            (ASK, capital_question()),
            ("noesis.learner_read", json!({})),
        ],
    );
    engine.script(Agent::Prober, "never reached", vec![]);
    let mut t = Teaching::new(&noesis, engine);
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    let reports = t.advance(&id).unwrap();
    assert_eq!(reports[0].end, TurnEnd::AwaitingAnswer);
    assert_eq!(reports[0].steps.len(), 1);
    assert!(reports[0].steps[0].calls.iter().all(|call| !call.is_error));
    assert_eq!(
        t.engine().remaining(Agent::Prober),
        1,
        "no model call after the pose"
    );
    let posed = session(&t, &id).open_question.expect("the pose is pending");

    // A restart rebuilds the pending question from the record alone.
    let engine = std::mem::take(t.engine_mut());
    drop(t);
    let restarted = Teaching::new(&noesis, engine);
    assert_eq!(session(&restarted, &id).open_question, Some(posed));
}

#[test]
fn t04_an_invalid_pose_returns_an_error_and_the_model_may_retry() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    let lonely = choice("Pick", &[("a", "Only")], &["a"]);
    t.engine_mut()
        .script(Agent::Prober, "", vec![(ASK, lonely)]);
    t.engine_mut()
        .script(Agent::Prober, "", vec![(ASK, capital_question())]);
    let reports = t.advance(&id).unwrap();
    let steps = &reports[0].steps;
    assert_eq!(steps.len(), 2);
    assert!(steps[0].calls[0].is_error);
    assert!(!steps[1].calls[0].is_error);
    assert_eq!(reports[0].end, TurnEnd::AwaitingAnswer);
    let results = t.engine().inputs_of(&reports[0].agent);
    assert!(
        matches!(results[1], Input::ToolResults(r) if r[0].is_error),
        "the refusal reached the model"
    );
}

#[test]
fn a_practitioner_refuses_a_graph_write_and_continues() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise sums", Mode::Practice, &[])
        .unwrap()
        .id;
    let available = tools::tools_for(Agent::Practitioner);
    assert!(available.contains(&tools::ToolName::GraphSearch));
    assert!(
        !available
            .iter()
            .any(|tool| tool.as_str().contains("extract"))
    );
    t.engine_mut().script(
        Agent::Practitioner,
        "",
        vec![("noesis.extract", json!({"session": id}))],
    );
    let problem = json!({"statement": "Add two and three.", "mode": "recall", "solution": "five"});
    t.engine_mut()
        .script(Agent::Practitioner, "", vec![(PROBLEM, problem)]);
    let report = t.advance(&id).unwrap().remove(0);
    assert!(report.steps[0].calls[0].is_error);
    assert!(!report.steps[1].calls[0].is_error);
    assert_eq!(report.end, TurnEnd::AwaitingLearner);
    assert!(session(&t, &id).active_problem.is_some());
}

#[test]
fn t05_question_authoring_reports_every_problem() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let mut broken = choice("Pick", &[("a", "Only")], &["z"]);
    broken["plan_node"] = json!("nowhere");
    let error = error_of(&call(&mut t, &id, ASK, broken, ""));
    assert!(error.contains("2 to 12 options"), "{error}");
    assert!(error.contains("exact label"), "{error}");
    assert!(error.contains("not a node of the current plan"), "{error}");

    let keyless = json!({"kind": "recall", "prompt": "Define a limit"});
    assert!(error_of(&call(&mut t, &id, ASK, keyless, "")).contains("committed `answer_key`"));
    // Labels differ from the plan's titles, which a pose may not repeat.
    let two_keys = choice(
        "Pick",
        &[("opt1", "Alpha"), ("opt2", "Beta")],
        &["opt1", "opt2"],
    );
    let mut single = two_keys.clone();
    single["multi"] = json!(false);
    assert!(error_of(&call(&mut t, &id, ASK, single, "")).contains("exactly one"));
    let leak = "Hint: the answer is Paris.";
    assert!(error_of(&call(&mut t, &id, ASK, capital_question(), leak)).contains("gives away"));
    let recall_leak = "Remember: the derivative of x^2 is 2x";
    assert!(
        error_of(&call(
            &mut t,
            &id,
            ASK,
            recall("d/dx of x^2?", "2x"),
            recall_leak
        ))
        .contains("gives away")
    );
    assert!(
        session(&t, &id).open_question.is_none(),
        "no refused pose is stored"
    );

    assert!(
        !call(&mut t, &id, ASK, two_keys.clone(), "Which apply?")
            .result
            .is_error
    );
    assert!(session(&t, &id).open_question.unwrap().multi);
    assert_eq!(
        answer_choice(&mut t, &id, &["Alpha"]).correct,
        Some(false),
        "a subset is wrong"
    );
    assert!(!call(&mut t, &id, ASK, two_keys, "").result.is_error);
    assert_eq!(
        answer_choice(&mut t, &id, &["Beta", "Alpha"]).correct,
        Some(true),
        "the exact set is right"
    );
}

#[test]
fn t06_options_shuffle_deterministically() {
    let options: Vec<ChoiceOption> = ["a", "b", "c", "d"]
        .iter()
        .map(|v| ChoiceOption {
            value: v.to_string(),
            label: v.to_uppercase(),
        })
        .collect();
    assert_eq!(shuffle("q_1", &options), shuffle("q_1", &options));
    let mut sorted = shuffle("q_1", &options);
    sorted.sort_by(|x, y| x.value.cmp(&y.value));
    assert_eq!(sorted, options, "a shuffle is a permutation");
    assert!((0..32).any(|n| shuffle(&format!("q_{n}"), &options) != options));
}

#[test]
fn rekey_preserves_the_correct_option_in_both_orders_and_id_bases() {
    for ids in [["0", "1"], ["1", "2"]] {
        let authored = vec![
            ChoiceOption {
                value: ids[0].into(),
                label: "Square".into(),
            },
            ChoiceOption {
                value: ids[1].into(),
                label: "Circle".into(),
            },
        ];
        for options in [authored.clone(), authored.iter().rev().cloned().collect()] {
            let (shown, key) = rekey(&options, &[ids[0].into()]);
            let square = shown
                .iter()
                .find(|option| option.label == "Square")
                .unwrap();
            let circle = shown
                .iter()
                .find(|option| option.label == "Circle")
                .unwrap();
            assert_eq!(key.len(), 1);
            assert_eq!(key[0], square.value);
            let posed = question::Posed {
                kind: QuestionKind::Choice,
                multi: false,
                options: &shown,
                answer_key: &key,
            };
            for (option, expected) in [(square, true), (circle, false)] {
                let answer = Answer {
                    choices: vec![option.value.clone()],
                    ..Answer::default()
                };
                assert_eq!(
                    question::grade(&posed, &answer).unwrap().correct,
                    Some(expected)
                );
            }
        }
    }
}

#[test]
fn choice_key_tracks_its_label_through_shuffle_and_answer() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn shapes", Mode::Teach, &[])
        .unwrap()
        .id;
    for values in [["0", "1", "2"], ["1", "2", "3"]] {
        for correct in [false, true] {
            let input = json!({"kind": "choice", "prompt": "Which shape has four equal sides?",
                "options": [{"value": values[0], "label": "Square"},
                            {"value": values[1], "label": "Triangle"},
                            {"value": values[2], "label": "Circle"}],
                "answer_key": ["Square"], "explanation": "Four equal sides form a square."});
            assert!(!call(&mut t, &id, ASK, input, "").result.is_error);
            let posed = session(&t, &id).open_question.unwrap();
            let wire = serde_json::to_string(&posed).unwrap();
            assert!(!wire.contains("answer_key"));
            assert!(!wire.contains("Four equal sides form a square."));
            let options = posed.options.as_ref().unwrap();
            let picked = if correct { "Square" } else { "Triangle" };
            let chosen = options
                .iter()
                .find(|option| option.label == picked)
                .unwrap();
            let answer = Answer {
                choices: vec![chosen.value.clone()],
                ..Answer::default()
            };
            let result = t.answer(&id, &posed.id, &answer).unwrap();
            assert_eq!(result.correct, Some(correct));
            let square = options
                .iter()
                .find(|option| option.label == "Square")
                .unwrap();
            assert_eq!(
                result.answer_key,
                Some(RevealedKey::Choices(vec![square.value.clone()]))
            );
        }
    }
}

#[test]
fn stale_choice_key_is_refused_until_it_names_the_new_label() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn shapes", Mode::Teach, &[])
        .unwrap()
        .id;
    let mut question = choice(
        "Which shape has four equal sides?",
        &[("opt1", "Square"), ("opt2", "Circle")],
        &["opt1"],
    );
    question["answer_key"] = json!(["opt2"]);
    let error = error_of(&call(&mut t, &id, ASK, question.clone(), ""));
    assert!(error.contains("exact label"));
    assert!(session(&t, &id).open_question.is_none());
    question["answer_key"] = json!(["Square"]);
    assert!(!call(&mut t, &id, ASK, question, "").result.is_error);
    assert_eq!(answer_choice(&mut t, &id, &["Square"]).correct, Some(true));
}

#[test]
fn t06_answers_grade_locally_with_confidence_stops() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    assert!(
        !call(&mut t, &id, ASK, capital_question(), "")
            .result
            .is_error
    );
    let posed = session(&t, &id).open_question.unwrap();
    let authored = [("a", "Paris"), ("b", "Rome"), ("c", "Madrid")].map(|(v, l)| ChoiceOption {
        value: v.into(),
        label: l.into(),
    });
    let (shown, key) = rekey(&shuffle(&posed.id, &authored), &["a".to_string()]);
    assert_eq!(posed.options.clone().unwrap(), shown);
    assert_eq!(
        key,
        shown_values(&t, &id, &["Paris"]),
        "the key follows its option"
    );

    let bad = Answer {
        choices: key.clone(),
        confidence: Some(45),
        ..Answer::default()
    };
    assert!(matches!(
        t.answer(&id, &posed.id, &bad),
        Err(TeachError::Invalid(_))
    ));
    let both = Answer {
        dont_know: true,
        confidence: Some(50),
        ..Answer::default()
    };
    assert!(matches!(
        t.answer(&id, &posed.id, &both),
        Err(TeachError::Invalid(_))
    ));
    let steps_before = t.engine().steps_run;
    let plain = Answer {
        choices: key.clone(),
        ..Answer::default()
    };
    let result = t.answer(&id, &posed.id, &plain).unwrap();
    assert_eq!(result.correct, Some(true));
    assert_eq!(result.answer_key, Some(RevealedKey::Choices(key)));
    assert_eq!(
        t.engine().steps_run,
        steps_before,
        "grading made no model call"
    );
    let evidence = t.take_evidence();
    assert_eq!(evidence[0].confidence, Some(DEFAULT_CONFIDENCE));
    assert_eq!(evidence[0].kind, EvidenceKind::ProbeAnswer);
}

#[test]
fn dont_know_never_counts_as_mastery_evidence() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    t.take_evidence();
    let dont_know = Answer {
        dont_know: true,
        ..Answer::default()
    };
    let mut check = capital_question();
    check["plan_node"] = json!("a");
    let mut recalled = recall("d/dx of x^2?", "2x");
    recalled["plan_node"] = json!("b");
    for question in [check, recalled] {
        assert!(!call(&mut t, &id, ASK, question, "").result.is_error);
        let open = session(&t, &id).open_question.unwrap();
        let result = t.answer(&id, &open.id, &dont_know).unwrap();
        assert_eq!(result.correct, None, "don't know is never graded");
        assert!(result.answer_key.is_some(), "the key is still revealed");
    }
    let evidence = t.take_evidence();
    assert_eq!(evidence.len(), 2);
    for draft in &evidence {
        assert_eq!(draft.outcome, EvidenceOutcome::DontKnow);
        assert_eq!(draft.confidence, None);
        assert!(draft.dont_know && !draft.supports_mastery());
    }
    let stored: Vec<Option<bool>> = noesis
        .store()
        .connection()
        .prepare("SELECT correct FROM questions WHERE plan_node IS NOT NULL")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(stored, vec![None, None]);
}

#[test]
fn t07_teach_checks_collapse_and_predictions_probes_and_goals_never_do() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let probe = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    assert!(
        !call(&mut t, &probe, ASK, capital_question(), "")
            .result
            .is_error
    );
    assert!(
        !session(&t, &probe).open_question.unwrap().collapsed,
        "a probe"
    );

    let id = teacher_session(&mut t);
    let goal = json!({"kind": "goal", "prompt": "Shall we start?"});
    let mut predict = capital_question();
    predict["intent"] = json!("predict");
    predict["plan_node"] = json!("a");
    let mut check = capital_question();
    check["plan_node"] = json!("a");
    for (question, collapsed) in [(goal, false), (predict, false), (check, true)] {
        assert!(!call(&mut t, &id, ASK, question, "").result.is_error);
        let open = session(&t, &id).open_question.unwrap();
        assert_eq!(open.collapsed, collapsed, "{:?}", open.kind);
        let answer = match open.kind {
            QuestionKind::Goal => Answer {
                text: Some("yes".into()),
                ..Answer::default()
            },
            _ => Answer {
                choices: shown_values(&t, &id, &["Paris"]),
                ..Answer::default()
            },
        };
        t.answer(&id, &open.id, &answer).unwrap();
    }
    let kinds: Vec<EvidenceKind> = t.take_evidence().iter().map(|draft| draft.kind).collect();
    assert_eq!(
        kinds,
        vec![
            EvidenceKind::MisconceptionObserved,
            EvidenceKind::PredictAnswer,
            EvidenceKind::CheckAnswer
        ]
    );
}

#[test]
fn t08_plan_validation_reports_every_problem_and_a_rewrite_replaces_the_plan() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let broken = json!({
        "nodes": [{"id": "Bad Slug", "title": "x"}, {"id": "a", "title": "A"}, {"id": "a", "title": "again"},
                  {"id": "b", "title": "B"}, {"id": "c", "title": "C"}],
        "edges": [{"from": "a", "to": "a"}, {"from": "b", "to": "c"}, {"from": "b", "to": "c"},
                  {"from": "c", "to": "b"}, {"from": "a", "to": "ghost"}],
    });
    let error = error_of(&call(&mut t, &id, PLAN, broken, ""));
    for expected in [
        "is a slug",
        "duplicate id",
        "its own prerequisite",
        "duplicate edge",
        "no node `ghost`",
        "cycle",
    ] {
        assert!(error.contains(expected), "missing `{expected}` in {error}");
    }
    assert_eq!(
        session(&t, &id).plan.unwrap().revision,
        1,
        "a refused plan changes nothing"
    );

    let rewrite = json!({"nodes": [{"id": "only-step", "title": "Only"}], "edges": []});
    assert!(!call(&mut t, &id, PLAN, rewrite, "").result.is_error);
    let plan = session(&t, &id).plan.unwrap();
    assert_eq!(plan.revision, 2);
    let ids: Vec<&str> = plan.nodes.iter().map(|node| node.id.as_str()).collect();
    assert_eq!(ids, vec!["only-step"]);
    assert!(plan.edges.is_empty());
}

#[test]
fn t09_a_passed_check_visits_its_node_and_the_next_step_is_current() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let statuses = |t: &Teaching<'_, FauxEngine>| -> Vec<NodeStatus> {
        session(t, &id)
            .plan
            .unwrap()
            .nodes
            .iter()
            .map(|node| node.status)
            .collect()
    };
    assert_eq!(statuses(&t), vec![NodeStatus::Upcoming; 3]);
    for node in ["a", "b"] {
        let mut check = capital_question();
        check["plan_node"] = json!(node);
        assert!(!call(&mut t, &id, ASK, check, "").result.is_error);
        answer_choice(&mut t, &id, &["Paris"]);
    }
    assert_eq!(
        statuses(&t),
        vec![
            NodeStatus::Visited,
            NodeStatus::Visited,
            NodeStatus::Current
        ]
    );
    let last_plan = t
        .take_events()
        .into_iter()
        .rev()
        .find_map(|event| match event {
            NoesisEvent::PlanUpdated { plan, .. } => Some(plan),
            _ => None,
        });
    assert_eq!(
        last_plan.unwrap().nodes[2].status,
        NodeStatus::Current,
        "the answer's event carries the status"
    );
}

fn problem(statement: &str, targets: Value, solution: &str) -> Value {
    json!({"statement": statement, "mode": "recall", "targets": targets, "solution": solution})
}

fn solved() -> Value {
    json!({"outcome": "solved", "rung": 0, "retest": false})
}

#[test]
fn t10_one_active_problem_and_targets_resolve_to_proposition_ids() {
    let (_dir, noesis) = fixture();
    add_proposition(&noesis, "p_chain", "Chain rule", "composition rule");
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session(
            "Practise derivatives",
            Mode::Practice,
            &["composition rule".into()],
        )
        .unwrap()
        .id;
    assert!(error_of(&call(&mut t, &id, CONCLUDE, solved(), "")).contains("no active problem"));
    let unknown = problem("Differentiate", json!(["no such idea"]), "x");
    assert!(error_of(&call(&mut t, &id, PROBLEM, unknown, "")).contains("no proposition is named"));

    assert!(
        !call(
            &mut t,
            &id,
            PROBLEM,
            problem("d/dx sin(x^2)?", json!(["chain RULE"]), "2x cos(x^2)"),
            ""
        )
        .result
        .is_error
    );
    assert_eq!(
        session(&t, &id).active_problem.unwrap().targets,
        vec!["p_chain".to_string()]
    );
    let second = problem("Another", json!(["p_chain"]), "y");
    assert!(error_of(&call(&mut t, &id, PROBLEM, second, "")).contains("already active"));

    let next = problem("d/dx e^(3x)?", json!(["composition rule"]), "3e^(3x)");
    t.engine_mut().script(
        Agent::Practitioner,
        "Well done. Next:",
        vec![(CONCLUDE, solved()), (PROBLEM, next)],
    );
    let report = t.run_turn(&id).unwrap();
    assert!(
        report.steps[0].calls.iter().all(|call| !call.is_error),
        "{:?}",
        report.steps[0].calls
    );
    let active = session(&t, &id).active_problem.unwrap();
    assert_eq!(active.statement, "d/dx e^(3x)?");
    assert_eq!(active.targets, vec!["p_chain".to_string()]);
}

#[test]
fn t11_conclude_checks_its_enums_and_records_evidence() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise derivatives", Mode::Practice, &[])
        .unwrap()
        .id;
    assert!(
        !call(
            &mut t,
            &id,
            PROBLEM,
            problem("d/dx x^3?", json!([]), "3x^2"),
            ""
        )
        .result
        .is_error
    );
    let bad = json!({"outcome": "won", "rung": 6, "miss_class": "lazy"});
    let error = error_of(&call(&mut t, &id, CONCLUDE, bad, ""));
    for expected in [
        "`outcome` must be one of",
        "`rung` must be a hint rung",
        "`miss_class` must be one of",
        "`retest` is required",
    ] {
        assert!(error.contains(expected), "missing `{expected}` in {error}");
    }
    let hintless = json!({"outcome": "solved_with_hints", "retest": true});
    assert!(error_of(&call(&mut t, &id, CONCLUDE, hintless, "")).contains("highest hint rung"));
    let done =
        json!({"outcome": "surrendered", "rung": 5, "miss_class": "structural", "retest": true});
    assert!(!call(&mut t, &id, CONCLUDE, done, "").result.is_error);
    let evidence = t.take_evidence();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].kind, EvidenceKind::PracticeConclude);
    assert_eq!(evidence[0].outcome, EvidenceOutcome::Incorrect);
    assert_eq!(
        (evidence[0].hint_rung, evidence[0].miss_class),
        (Some(5), Some(MissClass::Structural))
    );
}

fn graph_rows(noesis: &crate::Noesis) -> Vec<i64> {
    let conn = noesis.store().connection();
    [
        "propositions",
        "proposition_aliases",
        "proposition_provenance",
        "chains",
        "chain_premises",
        "clusters",
        "coverage",
    ]
    .iter()
    .map(|table| {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    })
    .collect()
}

#[test]
fn t11_the_solution_is_sealed_on_every_read_path_until_conclude_and_practice_never_writes_the_graph()
 {
    const SECRET: &str = "SECRET-SOLUTION-42";
    let (_dir, noesis) = fixture();
    add_proposition(&noesis, "p_power", "Power rule", "power law");
    let graph_before = graph_rows(&noesis);
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise derivatives", Mode::Practice, &[])
        .unwrap()
        .id;
    t.engine_mut().script(
        Agent::Practitioner,
        "Try this one.",
        vec![(PROBLEM, problem("d/dx x^5?", json!(["power law"]), SECRET))],
    );
    let report = t.run_turn(&id).unwrap();
    assert_eq!(report.steps[0].calls[0].input, json!(SEALED));
    t.send(&id, "is it 5x^4?").unwrap();
    let mut events = t.take_events();
    let evidence = t.take_evidence();
    let before = format!(
        "{}{}{report:?}{:?}{:?}{evidence:?}",
        serde_json::to_string(&events).unwrap(),
        serde_json::to_string(&session(&t, &id)).unwrap(),
        t.engine().inputs,
        t.engine().spawned,
    );
    assert!(
        !before.contains(SECRET),
        "the solution leaked before conclude"
    );

    t.engine_mut()
        .script(Agent::Practitioner, "Yes!", vec![(CONCLUDE, solved())]);
    let report = t.run_turn(&id).unwrap();
    assert!(
        !format!("{report:?}{:?}", t.engine().inputs).contains(SECRET),
        "the transcript stays sealed"
    );
    events.extend(t.take_events());
    let revealed = events.iter().find_map(|event| match event {
        NoesisEvent::PracticeConcluded {
            solution, outcome, ..
        } => Some((solution.clone(), *outcome)),
        _ => None,
    });
    assert_eq!(revealed, Some((SECRET.to_string(), Outcome::Solved)));
    let history = session(&t, &id).problems;
    let conclusion = history[0].conclusion.as_ref().expect("concluded");
    assert_eq!(
        (conclusion.solution.as_str(), conclusion.outcome),
        (SECRET, Outcome::Solved),
        "the session's history reveals the solution once concluded"
    );
    assert_eq!(
        graph_rows(&noesis),
        graph_before,
        "practice never writes the graph"
    );
}

#[test]
fn t11_answer_keys_are_sealed_until_the_answer_is_recorded() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    let mut secret = recall("Name the rule for f(g(x))", "KEY-CHAIN-RULE");
    secret["explanation"] = json!("EXPLAIN-OUTER-INNER");
    t.engine_mut()
        .script(Agent::Prober, "One question.", vec![(ASK, secret)]);
    let report = t.advance(&id).unwrap();
    let events = t.take_events();
    let before = format!(
        "{}{}{report:?}{:?}",
        serde_json::to_string(&events).unwrap(),
        serde_json::to_string(&session(&t, &id)).unwrap(),
        t.engine().inputs,
    );
    assert!(
        !before.contains("KEY-CHAIN-RULE") && !before.contains("EXPLAIN-OUTER-INNER"),
        "{before}"
    );
    let open = session(&t, &id).open_question.unwrap();
    let answer = Answer {
        text: Some("the chain rule".into()),
        ..Answer::default()
    };
    let result = t.answer(&id, &open.id, &answer).unwrap();
    assert_eq!(
        result.answer_key,
        Some(RevealedKey::Text("KEY-CHAIN-RULE".into()))
    );
    assert_eq!(result.explanation.as_deref(), Some("EXPLAIN-OUTER-INNER"));
    let asked = session(&t, &id).questions;
    assert_eq!(
        asked[0].result.as_ref(),
        Some(&result),
        "the history holds the reveal"
    );
    let inputs = &t.engine().inputs;
    let answers: Vec<_> = inputs
        .iter()
        .filter_map(|(_, input)| match input {
            Input::Answer(answer) => Some(answer),
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 1, "one answer is delivered");
    assert_eq!(answers[0]["answer_key"], "KEY-CHAIN-RULE");
    assert!(
        inputs
            .iter()
            .all(|(_, input)| matches!(input, Input::Answer(_))
                || !format!("{input:?}").contains("KEY-CHAIN-RULE"))
    );
    assert!(!format!("{inputs:?}").contains("EXPLAIN-OUTER-INNER"));
}

#[test]
fn t12_malformed_tool_arguments_are_healed() {
    let (_dir, noesis) = fixture();
    add_proposition(&noesis, "p_chain", "Chain rule", "composition rule");
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let teach = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    let messy_ask = json!({
        "kind": "\"choice\"", "prompt": "Capital of France?",
        "options": "[{\"value\": \"a\", \"label\": \"Paris\"}, {\"value\": \"b\", \"label\": \"Rome\"}]",
        "answer_key": "[\"Paris\"]", "multi": "false", "explanation": "why", "concepts": "geography",
    });
    assert!(!call(&mut t, &teach, ASK, messy_ask, "").result.is_error);
    let open = session(&t, &teach).open_question.unwrap();
    assert_eq!(
        (open.kind, open.multi, open.concepts.clone()),
        (QuestionKind::Choice, false, vec!["geography".to_string()])
    );

    let practice = t
        .create_session("Practise derivatives", Mode::Practice, &[])
        .unwrap()
        .id;
    let messy_problem = json!({"statement": "d/dx sin(2x)?", "mode": "'Recall'", "targets": "[\"Chain rule\"]", "solution": "2cos(2x)"});
    assert!(
        !call(&mut t, &practice, PROBLEM, messy_problem, "")
            .result
            .is_error
    );
    let messy_conclude = json!({"outcome": "'Solved With Hints'", "rung": "R2", "retest": "yes", "miss_class": null});
    assert!(
        !call(&mut t, &practice, CONCLUDE, messy_conclude, "")
            .result
            .is_error
    );
    let concluded = t.take_events().into_iter().find_map(|event| match event {
        NoesisEvent::PracticeConcluded {
            outcome,
            rung,
            retest,
            ..
        } => Some((outcome, rung, retest)),
        _ => None,
    });
    assert_eq!(concluded, Some((Outcome::SolvedWithHints, Some(2), true)));
}

#[test]
fn every_agent_runs_in_run_noesis_with_a_noesis_role() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    teacher_session(&mut t);
    t.create_session("Practise derivatives", Mode::Practice, &[])
        .unwrap();
    let spawned = &t.engine().spawned;
    assert_eq!(spawned.len(), 3);
    for (_, spec) in spawned {
        assert_eq!((spec.run, spec.subsystem), (NOESIS_RUN, SUBSYSTEM));
        assert_eq!(spec.role, role_of(spec.agent));
        assert!(spec.role.starts_with("noesis."));
        assert_eq!(
            spec.model, "faux/subscription",
            "an unset slot never takes the billed model"
        );
        assert!(
            spec.tools
                .iter()
                .all(|tool| tool.name.starts_with("noesis."))
        );
        assert!(!spec.system_prompt.is_empty());
    }
}

#[test]
fn tools_register_only_for_a_running_service_and_noesis_roles() {
    let (dir, noesis) = fixture();
    assert!(tools::registered_tools(None, "noesis.prober").is_empty());
    assert!(tools::registered_tools(Some(&noesis), "worker").is_empty());
    assert!(tools::registered_tools(Some(&noesis), "noesis.extractor").is_empty());
    let names: Vec<&str> = tools::registered_tools(Some(&noesis), "noesis.prober")
        .iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(
        names,
        vec![
            ASK,
            "noesis.judge_recall",
            PROBE_DONE,
            "noesis.graph_search",
            "noesis.learner_read"
        ]
    );
    let disabled = crate::Noesis::start_if_enabled(
        &NoesisSettings::default(),
        dir.path(),
        chrono::Local::now().date_naive(),
        LaneSpend::default(),
    );
    assert!(
        disabled.unwrap().is_none(),
        "disabled, there is no service to register tools for"
    );
}

#[test]
fn a_tool_from_a_non_noesis_role_or_a_stale_agent_is_refused() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    let agent = session(&t, &id).agent;
    let ask = ToolCall {
        id: "c".into(),
        name: ASK.into(),
        input: capital_question(),
    };
    let outsider = Caller {
        agent: agent.clone(),
        role: "worker".into(),
    };
    assert!(error_of(&t.call_tool(&id, &outsider, &ask, "").unwrap()).contains("noesis.* roles"));
    let wrong_tool = Caller {
        agent: agent.clone(),
        role: "noesis.prober".into(),
    };
    let plan = ToolCall {
        id: "c".into(),
        name: PLAN.into(),
        input: three_steps(),
    };
    assert!(error_of(&t.call_tool(&id, &wrong_tool, &plan, "").unwrap()).contains("not available"));
    let stranger = Caller {
        agent: "agt_other".into(),
        role: "noesis.prober".into(),
    };
    assert!(error_of(&t.call_tool(&id, &stranger, &ask, "").unwrap()).contains("no longer drives"));
}

#[test]
fn every_model_call_is_admitted_against_the_budget_first() {
    let mut settings = NoesisSettings::default();
    settings.budget.interactive_usd_daily = 0.0;
    let (_dir, noesis) = start(settings);
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    let error = t.advance(&id).unwrap_err();
    assert!(
        matches!(error, TeachError::Budget(Refusal::BudgetZero { .. })),
        "{error}"
    );
    assert_eq!(t.engine().steps_run, 0, "refused before any model call");

    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut()
        .script(Agent::Prober, "What do you already know?", vec![]);
    t.advance(&id).unwrap();
    let today = chrono::Local::now().date_naive();
    let spent = noesis
        .accountant()
        .spent_today(crate::budget::Lane::Interactive, today);
    assert!(spent.usd > 0.0 && spent.tokens == 100, "{spent:?}");
}

#[test]
fn a_session_never_falls_back_to_a_billed_model() {
    let (_dir, noesis) = fixture();
    let mut engine = FauxEngine::new();
    engine.api_only = true;
    let mut t = Teaching::new(&noesis, engine);
    let error = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap_err();
    assert!(matches!(error, TeachError::Model(_)), "{error}");
    assert!(t.engine().spawned.is_empty());
}

#[test]
fn operator_prompt_overrides_apply_only_with_the_switch_on() {
    for switch in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let prompts_dir = dir.path().join("noesis/prompts/prober");
        std::fs::create_dir_all(&prompts_dir).unwrap();
        std::fs::write(prompts_dir.join("01-role.md"), "OPERATOR ROLE TEXT").unwrap();
        let settings = NoesisSettings {
            enabled: true,
            prompt_overrides: switch,
            ..NoesisSettings::default()
        };
        let today = chrono::Local::now().date_naive();
        let noesis =
            crate::Noesis::start_if_enabled(&settings, dir.path(), today, LaneSpend::default())
                .unwrap()
                .unwrap();
        let mut t = Teaching::new(&noesis, FauxEngine::new());
        t.create_session("Learn derivatives", Mode::Teach, &[])
            .unwrap();
        let prompt = &t.engine().spawned[0].1.system_prompt;
        assert_eq!(prompt.contains("OPERATOR ROLE TEXT"), switch);
    }
}
