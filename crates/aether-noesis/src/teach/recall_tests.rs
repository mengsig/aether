use serde_json::json;

use super::faux::FauxEngine;
use super::records::{Answer, Mode};
use super::testkit::*;
use super::*;
use crate::prompts::Agent;

const JUDGE: &str = "noesis.judge_recall";

fn answered(t: &mut Teaching<'_, FauxEngine>) -> (String, String) {
    let id = t
        .create_session("Learn algebra", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![(ASK, recall("Solve 2x = 8", "x = 4"))],
    );
    assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::AwaitingAnswer);
    let question = session(t, &id).open_question.unwrap().id;
    let result = t
        .answer(
            &id,
            &question,
            &Answer {
                text: Some("x = 4".into()),
                thinking: Some("Divide both sides by 2".into()),
                ..Answer::default()
            },
        )
        .unwrap();
    assert_eq!(result.correct, None);
    (id, question)
}

#[test]
fn skipped_judgment_is_refused_then_corrected_before_the_next_question() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let (id, question) = answered(&mut t);
    t.engine_mut().script(
        Agent::Prober,
        "Next question",
        vec![(ASK, recall("What is 4 + 4?", "eight"))],
    );
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![
            (JUDGE, json!({"question": question, "correct": true})),
            (ASK, capital_question()),
        ],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    assert!(report.steps[0].calls[0].is_error);
    assert_eq!(report.steps[0].text, "", "an ask never shows prose");
    assert!(report.steps[1].calls.iter().all(|call| !call.is_error));
    let view = session(&t, &id);
    assert_eq!(
        view.questions[0].result.as_ref().unwrap().correct,
        Some(true)
    );
    assert!(view.open_question.is_some());
    assert!(t.engine().inputs_of(&view.agent).iter().any(
        |input| matches!(input, Input::Guidance(text) if text.contains("noesis.judge_recall"))
    ));
}

#[test]
fn no_tool_replies_are_bounded_and_leave_the_recall_recoverable() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let (id, question) = answered(&mut t);
    for _ in 0..3 {
        t.engine_mut()
            .script(Agent::Prober, "Let's move on", vec![]);
    }
    let before = t.engine().steps_run;
    let error = t.advance(&id).unwrap_err();
    assert!(
        error.to_string().contains("did not judge answered recall"),
        "{error}"
    );
    assert_eq!(t.engine().steps_run - before, 3);
    assert!(!t.turn_pending(&id).unwrap());
    let agent = session(&t, &id).agent;
    assert!(t.engine().inputs_of(&agent).iter().any(
        |input| matches!(input, Input::Failure(text) if text.contains("send a message to retry"))
    ));
    assert_eq!(
        session(&t, &id).questions[0]
            .result
            .as_ref()
            .unwrap()
            .correct,
        None
    );
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![(JUDGE, json!({"question": question, "correct": true}))],
    );
    t.engine_mut().script(Agent::Prober, "Well done", vec![]);
    t.send(&id, "Please retry").unwrap();
    assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::AwaitingLearner);
    assert_eq!(
        session(&t, &id).questions[0]
            .result
            .as_ref()
            .unwrap()
            .correct,
        Some(true)
    );
}

#[test]
fn handoff_waits_for_judgment_and_no_tools_gets_a_retry() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let (id, question) = answered(&mut t);
    t.engine_mut()
        .script(Agent::Prober, "Done", vec![(PROBE_DONE, strands())]);
    t.engine_mut().script(Agent::Prober, "Moving on", vec![]);
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![
            (JUDGE, json!({"question": question, "correct": false})),
            (PROBE_DONE, strands()),
        ],
    );
    t.engine_mut().script(
        Agent::Teacher,
        "Here is the plan",
        vec![(PLAN, three_steps())],
    );
    let reports = t.advance(&id).unwrap();
    assert_eq!(reports[0].end, TurnEnd::HandedOff);
    assert!(reports[0].steps[0].calls[0].is_error);
    assert_eq!(reports[0].steps[1].text, "Moving on");
    assert_eq!(
        session(&t, &id).questions[0]
            .result
            .as_ref()
            .unwrap()
            .correct,
        Some(false)
    );
}

#[test]
fn failed_model_call_preserves_obligation_for_the_next_advance() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let (id, question) = answered(&mut t);
    t.engine_mut()
        .fail_next(Agent::Prober, EngineError::NotSent("cancelled".into()));
    assert!(matches!(
        t.advance(&id),
        Err(TeachError::Engine(EngineError::NotSent(_)))
    ));
    assert_eq!(
        session(&t, &id).questions[0]
            .result
            .as_ref()
            .unwrap()
            .correct,
        None
    );
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![
            (JUDGE, json!({"question": question, "correct": true})),
            (ASK, capital_question()),
        ],
    );
    assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::AwaitingAnswer);
    assert_eq!(
        session(&t, &id).questions[0]
            .result
            .as_ref()
            .unwrap()
            .correct,
        Some(true)
    );
}

#[test]
fn dont_know_does_not_require_a_judgment() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn algebra", Mode::Teach, &[])
        .unwrap()
        .id;
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![(ASK, recall("Solve 2x = 8", "x = 4"))],
    );
    assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::AwaitingAnswer);
    let question = session(&t, &id).open_question.unwrap().id;
    t.answer(
        &id,
        &question,
        &Answer {
            dont_know: true,
            ..Answer::default()
        },
    )
    .unwrap();
    t.engine_mut()
        .script(Agent::Prober, "", vec![(ASK, capital_question())]);
    assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::AwaitingAnswer);
    let view = session(&t, &id);
    assert_eq!(view.questions[0].result.as_ref().unwrap().correct, None);
    assert_eq!(view.questions.len(), 2);
}

#[test]
fn budget_interruption_and_store_restart_preserve_the_obligation() {
    let mut settings = crate::settings::NoesisSettings::default();
    settings.budget.interactive_usd_daily = 0.013;
    let (dir, noesis) = start(settings.clone());
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let (id, question) = answered(&mut t);
    t.engine_mut().script(
        Agent::Prober,
        "Next",
        vec![(ASK, recall("What is 4 + 4?", "eight"))],
    );
    assert!(matches!(t.advance(&id), Err(TeachError::Budget(_))));
    assert!(session(&t, &id).stalled.is_some());
    assert_eq!(
        session(&t, &id).questions[0]
            .result
            .as_ref()
            .unwrap()
            .correct,
        None
    );
    let (engine, _) = t.into_parts();
    drop(noesis);

    settings.enabled = true;
    settings.budget.interactive_usd_daily = 1.0;
    let noesis = crate::Noesis::start_if_enabled(
        &settings,
        dir.path(),
        chrono::Local::now().date_naive(),
        Default::default(),
    )
    .unwrap()
    .unwrap();
    let mut t = Teaching::new(&noesis, engine);
    t.retry_turn(&id).unwrap();
    t.engine_mut().script(
        Agent::Prober,
        "",
        vec![
            (JUDGE, json!({"question": question, "correct": true})),
            (ASK, capital_question()),
        ],
    );
    assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::AwaitingAnswer);
    assert_eq!(
        session(&t, &id).questions[0]
            .result
            .as_ref()
            .unwrap()
            .correct,
        Some(true)
    );
}

#[test]
fn judgment_accepts_prompt_or_omitted_id_and_refuses_other_ids() {
    const PROMPT: &str = "A rectangle has length 8 and width 3. What is its perimeter, and what calculation did you use?";
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    for supplied in [Some(PROMPT), None] {
        let id = t
            .create_session("Learn perimeter", Mode::Teach, &[])
            .unwrap()
            .id;
        t.engine_mut()
            .script(Agent::Prober, "", vec![(ASK, recall(PROMPT, "22"))]);
        assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::AwaitingAnswer);
        let question = session(&t, &id).open_question.unwrap().id;
        t.answer(
            &id,
            &question,
            &Answer {
                text: Some("22".into()),
                ..Answer::default()
            },
        )
        .unwrap();
        let wrong = call(
            &mut t,
            &id,
            JUDGE,
            json!({"question": "q_other", "correct": false}),
            "",
        );
        assert!(wrong.result.is_error);
        assert!(
            wrong.result.content["error"]
                .as_str()
                .unwrap()
                .contains(&question)
        );
        let result = session(&t, &id).questions[0].result.clone().unwrap();
        assert_eq!(result.correct, None);
        assert!(
            call(
                &mut t,
                &id,
                JUDGE,
                json!({"question": "", "correct": false}),
                ""
            )
            .result
            .is_error
        );
        let args = match supplied {
            Some(prompt) => json!({"question": prompt, "correct": true}),
            None => json!({"correct": true}),
        };
        assert!(!call(&mut t, &id, JUDGE, args.clone(), "").result.is_error);
        assert!(call(&mut t, &id, JUDGE, args, "").result.is_error);
        let result = session(&t, &id).questions[0].result.clone().unwrap();
        assert_eq!(result.correct, Some(true));
    }
}

#[test]
fn teacher_judges_the_perimeter_answer_by_its_prompt_and_continues() {
    const PROMPT: &str = "A rectangle has length 8 and width 3. What is its perimeter, and what calculation did you use?";
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let mut first = recall(
        PROMPT,
        "22 units; add all four sides: 8+3+8+3, or use 2(8+3).",
    );
    first["plan_node"] = json!("a");
    t.engine_mut()
        .script(Agent::Teacher, "", vec![(ASK, first)]);
    assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::AwaitingAnswer);
    let question = session(&t, &id).open_question.unwrap().id;
    t.answer(
        &id,
        &question,
        &Answer {
            text: Some("22 units, because 8 + 3 + 8 + 3 = 2(8 + 3) = 22.".into()),
            ..Answer::default()
        },
    )
    .unwrap();
    let mut next = recall(
        "A rectangle is 5 units long and 4 units wide. What is its area, and what unit would you use?",
        "20 square units",
    );
    next["plan_node"] = json!("b");
    t.engine_mut().script(
        Agent::Teacher,
        "",
        vec![
            (JUDGE, json!({"question": PROMPT, "correct": true})),
            (ASK, next),
        ],
    );
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    assert!(report.steps[0].calls.iter().all(|call| !call.is_error));
    let view = session(&t, &id);
    assert_eq!(
        view.questions[0].result.as_ref().unwrap().correct,
        Some(true)
    );
    assert!(view.open_question.is_some());
}

#[test]
fn a_judged_recall_also_continues_to_the_next_step() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let mut first = recall("What is 2 + 2?", "four");
    first["plan_node"] = json!("a");
    t.engine_mut()
        .script(Agent::Teacher, "", vec![(ASK, first)]);
    t.advance(&id).unwrap();
    let question = session(&t, &id).open_question.unwrap().id;
    t.answer(
        &id,
        &question,
        &Answer {
            text: Some("four".into()),
            ..Answer::default()
        },
    )
    .unwrap();
    let mut next = recall("What is 3 + 3?", "six");
    next["plan_node"] = json!("b");
    t.engine_mut()
        .script(Agent::Teacher, "", vec![(JUDGE, json!({"correct": true}))]);
    t.engine_mut()
        .script(Agent::Teacher, "Are you ready?", vec![]);
    t.engine_mut().script(Agent::Teacher, "", vec![(ASK, next)]);
    let report = t.advance(&id).unwrap().remove(0);
    assert_eq!(report.end, TurnEnd::AwaitingAnswer);
    assert!(report.steps[1].text.is_empty());
    assert_eq!(
        session(&t, &id).questions[0]
            .result
            .as_ref()
            .unwrap()
            .correct,
        Some(true)
    );
}
