//! A session's durable history as `get_session` reads it: its agents and the
//! inputs Noesis wrote, every question and problem with its reveal, a stalled
//! turn, and the plan moving on as checks are passed.

use serde_json::json;

use super::faux::FauxEngine;
use super::records::*;
use super::testkit::*;
use super::*;
use crate::budget::INTERACTIVE_BUDGET_SETTING;
use crate::prompts::Agent;
use crate::settings::NoesisSettings;

/// The input the faux engine recorded at `seq`, with the agent it went to.
fn input_at(t: &Teaching<'_, FauxEngine>, seq: u64) -> &(String, Input) {
    let index = usize::try_from(seq - 1).unwrap();
    &t.engine().inputs[index]
}

#[test]
fn every_agent_is_listed_in_order_with_only_the_inputs_noesis_wrote_for_it() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    t.send(&id, "can we start with an example?").unwrap();
    let view = session(&t, &id);
    let roles: Vec<SessionRole> = view.agents.iter().map(|agent| agent.role).collect();
    assert_eq!(roles, [SessionRole::Prober, SessionRole::Teacher]);
    assert_eq!(view.agents[1].agent, view.agent, "the teacher drives now");
    let [prober, teacher] = [&view.agents[0], &view.agents[1]];
    assert_eq!((prober.internal.len(), teacher.internal.len()), (1, 1));
    let (to, opening) = input_at(&t, prober.internal[0]);
    assert!(
        to == &prober.agent
            && matches!(opening, Input::Opening(text) if text.contains("Learn derivatives"))
    );
    let (to, handoff) = input_at(&t, teacher.internal[0]);
    assert!(to == &teacher.agent && matches!(handoff, Input::Handoff(_)));
    let learner = t.engine().inputs.iter().position(
        |(_, input)| matches!(input, Input::Learner(text) if text.starts_with("can we start")),
    );
    let learner_seq = u64::try_from(learner.expect("sent") + 1).unwrap();
    assert!(
        !view
            .agents
            .iter()
            .any(|agent| agent.internal.contains(&learner_seq)),
        "the learner's own words are not internal"
    );

    let listed = t.list_sessions(None, 10).unwrap().sessions;
    assert!(
        listed[0].agents.is_empty() && listed[0].questions.is_empty(),
        "a list stays light"
    );
}

#[test]
fn every_question_is_kept_with_its_answer_and_only_an_answered_one_is_revealed() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let mut check = capital_question();
    check["plan_node"] = json!("a");
    check["explanation"] = json!("EXPLAIN-CAPITAL-987");
    assert!(!call(&mut t, &id, ASK, check, "").result.is_error);
    let open = session(&t, &id);
    assert_eq!(open.questions.len(), 1);
    assert!(open.questions[0].answer.is_none() && open.questions[0].result.is_none());
    let read = serde_json::to_string(&open).unwrap();
    assert!(!read.contains("EXPLAIN-CAPITAL-987"), "{read}");

    let result = answer_choice(&mut t, &id, &["Paris"]);
    assert!(
        !call(&mut t, &id, ASK, recall("Name the rule", "KEY-OPEN"), "")
            .result
            .is_error
    );
    let view = session(&t, &id);
    let asked = &view.questions;
    assert_eq!(asked.len(), 2, "oldest first, the open one last");
    let answer = asked[0].answer.as_ref().expect("answered");
    assert_eq!(
        (answer.chosen.clone(), answer.confidence),
        (vec!["Paris".to_string()], Some(70))
    );
    assert_eq!(asked[0].result.as_ref(), Some(&result));
    assert_eq!(asked[0].question.step, Some(QuestionStep::Check));
    assert!(asked[1].answer.is_none() && asked[1].result.is_none());
    let read = serde_json::to_string(&view).unwrap();
    assert!(
        !read.contains("KEY-OPEN"),
        "the open key stays sealed: {read}"
    );
}

#[test]
fn every_problem_is_kept_and_its_solution_appears_only_once_concluded() {
    const SOLUTION: &str = "SOLUTION-POWER-5X4";
    let (_dir, noesis) = fixture();
    add_proposition(&noesis, "p_power", "Power rule", "power law");
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Practise derivatives", Mode::Practice, &[])
        .unwrap()
        .id;
    let posed = json!({"statement": "d/dx x^5?", "mode": "recall", "targets": ["power law"], "solution": SOLUTION});
    t.engine_mut()
        .script(Agent::Practitioner, "Try this.", vec![(PROBLEM, posed)]);
    t.run_turn(&id).unwrap();
    let active = session(&t, &id);
    assert_eq!(active.problems.len(), 1);
    assert!(active.problems[0].conclusion.is_none());
    assert!(!serde_json::to_string(&active).unwrap().contains(SOLUTION));

    let solved = json!({"outcome": "solved", "rung": 1, "retest": false});
    t.engine_mut()
        .script(Agent::Practitioner, "Yes.", vec![(CONCLUDE, solved)]);
    t.run_turn(&id).unwrap();
    let problems = session(&t, &id).problems;
    let conclusion = problems[0].conclusion.as_ref().expect("concluded");
    assert_eq!(
        (
            conclusion.outcome,
            conclusion.rung,
            conclusion.solution.as_str()
        ),
        (Outcome::Solved, Some(1), SOLUTION)
    );
}

#[test]
fn a_spent_budget_stalls_the_session_until_a_turn_runs() {
    let mut settings = NoesisSettings::default();
    settings.budget.interactive_usd_daily = 0.0;
    let (_dir, noesis) = start(settings);
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    t.take_events();
    assert!(t.advance(&id).is_err());
    let stall = session(&t, &id).stalled.expect("stalled");
    assert_eq!(stall.reason, StallReason::BudgetExhausted);
    assert!(
        stall.message.contains(INTERACTIVE_BUDGET_SETTING),
        "{}",
        stall.message
    );
    let events = t.take_events();
    assert!(
        matches!(&events[..], [NoesisEvent::SessionStalled { stall: announced, .. }] if *announced == stall),
        "{events:?}"
    );

    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session("Learn derivatives", Mode::Teach, &[])
        .unwrap()
        .id;
    let spent = format!("spent; raise {INTERACTIVE_BUDGET_SETTING} to continue today");
    t.engine_mut()
        .fail_next(Agent::Prober, EngineError::BudgetSpent(spent.clone()))
        .script(Agent::Prober, "What do you know?", vec![]);
    assert!(t.advance(&id).is_err());
    assert_eq!(
        session(&t, &id).stalled.map(|stall| stall.message),
        Some(spent)
    );
    t.advance(&id).unwrap();
    assert_eq!(session(&t, &id).stalled, None, "a turn that ran clears it");
}

#[test]
fn the_plan_moves_on_only_when_a_check_is_passed() {
    let (_dir, noesis) = fixture();
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = teacher_session(&mut t);
    let statuses = |t: &Teaching<'_, FauxEngine>| -> Vec<NodeStatus> {
        let plan = session(t, &id).plan.expect("a plan");
        plan.nodes.iter().map(|node| node.status).collect()
    };
    use NodeStatus::{Current, Upcoming, Visited};
    let mut check = capital_question();
    check["plan_node"] = json!("a");
    assert!(!call(&mut t, &id, ASK, check.clone(), "").result.is_error);
    assert_eq!(statuses(&t), [Current, Upcoming, Upcoming]);
    answer_choice(&mut t, &id, &["Rome"]);
    assert_eq!(statuses(&t), [Current, Upcoming, Upcoming], "a miss stays");

    assert!(!call(&mut t, &id, ASK, check, "").result.is_error);
    t.take_events();
    answer_choice(&mut t, &id, &["Paris"]);
    assert_eq!(statuses(&t), [Visited, Current, Upcoming]);
    let announced = t.take_events().into_iter().find_map(|event| match event {
        NoesisEvent::PlanUpdated { plan, .. } => Some(plan),
        _ => None,
    });
    assert_eq!(
        announced.expect("the answer announces the plan").nodes[1].status,
        Current
    );

    for node in ["b", "c"] {
        let mut check = capital_question();
        check["plan_node"] = json!(node);
        assert!(!call(&mut t, &id, ASK, check, "").result.is_error);
        answer_choice(&mut t, &id, &["Paris"]);
    }
    assert_eq!(statuses(&t), [Visited; 3], "every step done");
}
