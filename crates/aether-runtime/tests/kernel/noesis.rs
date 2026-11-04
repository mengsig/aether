//! The Noesis guard: a Noesis agent takes no turn, and no command gives it
//! work, unless the gate is Enabled and the budget admits it.
use super::*;

use aether_core::agent::WaitCondition;
use aether_core::model::Usage;
use aether_runtime::RuntimeError;
use aether_runtime::noesis_guard::{
    NOESIS_SUBSYSTEM, NoesisBudget, NoesisCharge, NoesisGate, NoesisLane, NoesisRefusal,
};

/// What the recording budget answers each admit with.
#[derive(Debug, Clone, Copy)]
pub(super) enum Answer {
    Admit,
    BusyThenAdmit(usize),
    Exhausted,
}

/// A budget that records every admit and every settled charge.
#[derive(Debug)]
pub(super) struct Recording {
    answer: Answer,
    /// Drained as a turn is admitted: the gate leaves Enabled before its model call.
    drain_on_admit: Option<NoesisGate>,
    pub(super) admits: std::sync::Mutex<Vec<(AgentId, NoesisLane)>>,
    pub(super) settled: Arc<std::sync::Mutex<Vec<(AgentId, Usage)>>>,
}

impl Recording {
    pub(super) fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            answer,
            drain_on_admit: None,
            admits: Default::default(),
            settled: Default::default(),
        })
    }

    /// Admits every turn, draining `gate` as it does.
    pub(super) fn draining(gate: &NoesisGate) -> Arc<Self> {
        Arc::new(Self {
            answer: Answer::Admit,
            drain_on_admit: Some(gate.clone()),
            admits: Default::default(),
            settled: Default::default(),
        })
    }

    pub(super) fn admits(&self) -> Vec<(AgentId, NoesisLane)> {
        self.admits.lock().unwrap().clone()
    }

    pub(super) fn settled(&self) -> Vec<(AgentId, Usage)> {
        self.settled.lock().unwrap().clone()
    }
}

struct Charge {
    agent: AgentId,
    settled: Arc<std::sync::Mutex<Vec<(AgentId, Usage)>>>,
}

impl NoesisCharge for Charge {
    fn settle(self: Box<Self>, usage: &Usage) {
        self.settled.lock().unwrap().push((self.agent, *usage));
    }
}

impl NoesisBudget for Recording {
    fn admit(
        &self,
        agent: &Agent,
        lane: NoesisLane,
    ) -> Result<Box<dyn NoesisCharge>, NoesisRefusal> {
        let mut admits = self.admits.lock().unwrap();
        admits.push((agent.id, lane));
        match self.answer {
            Answer::Exhausted => return Err(NoesisRefusal::Exhausted("spent".into())),
            Answer::BusyThenAdmit(n) if admits.len() <= n => {
                return Err(NoesisRefusal::Busy("cap full".into()));
            }
            Answer::Admit | Answer::BusyThenAdmit(_) => {}
        }
        if let Some(gate) = &self.drain_on_admit {
            gate.drain();
        }
        Ok(Box::new(Charge {
            agent: agent.id,
            settled: self.settled.clone(),
        }))
    }
}

/// A harness whose runtime reads `gate`, with `extra_tools` registered.
pub(super) async fn noesis_harness(
    responses: Vec<aether_model::testing::ScriptedResponse>,
    gate: &NoesisGate,
    seed: impl FnOnce(&dyn Store),
    extra_tools: impl FnOnce(&mut ToolRegistry),
) -> Harness {
    let gate = gate.clone();
    harness_with_tools(responses, seed, |c| c.noesis = gate, extra_tools).await
}

pub(super) fn noesis_spec(role: &str, objective: &str) -> CreateAgentSpec {
    let mut s = spec(objective);
    s.role = role.into();
    s.subsystem = Some(NOESIS_SUBSYSTEM.into());
    s
}

/// A Noesis agent row in `status`, written straight to the store as a
/// previous daemon would have left it.
pub(super) fn seed_noesis(store: &dyn Store, role: &str, status: AgentStatus) -> AgentId {
    let at = now();
    let id = AgentId::new();
    let mut agent = Agent {
        archived: false,
        id,
        parent: None,
        role: role.into(),
        name: "n".into(),
        scope: Scope::default(),
        model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
        fallbacks: Vec::new(),
        status: AgentStatus::Created,
        paused: false,
        paused_via: None,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        engine_alive: false,
        last_event_at: None,
        config: AgentConfig {
            approval: ApprovalPolicy::none(),
            ..Default::default()
        },
        stats: Default::default(),
        run: Default::default(),
        created_at: at,
        updated_at: at,
    };
    for next in [AgentStatus::Ready, AgentStatus::Running] {
        if agent.status != status {
            agent.transition(next, at).expect("seed transition");
        }
    }
    if agent.status != status {
        agent.transition(status.clone(), at).expect("seeded status");
    }
    let mut commit = Commit::new()
        .event(EventDraft::new(Actor::System, EventKind::AgentStarted { turn: 1 }).agent(id))
        .mutate(Mutation::CreateAgent {
            agent,
            subsystem: Some(NOESIS_SUBSYSTEM.into()),
        });
    if status == AgentStatus::Running {
        commit.push_mutation(Mutation::SetLease(Lease::new(
            id,
            "dead-daemon",
            1,
            at,
            Duration::from_secs(1),
        )));
    }
    store.commit(commit).expect("seed");
    id
}

fn agent(h: &Harness, id: AgentId) -> Agent {
    h.store.agent(id).expect("read").expect("agent")
}

#[tokio::test]
async fn a_noesis_agent_with_work_is_parked_without_a_model_call_once_disabled() {
    let gate = NoesisGate::new();
    let budget = Recording::new(Answer::BusyThenAdmit(usize::MAX));
    gate.enable(budget.clone());
    let h = noesis_harness(
        vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "done"}),
        )],
        &gate,
        |_| {},
        |_| {},
    )
    .await;
    let (tutor, _) = h
        .runtime
        .create_agent(noesis_spec("noesis.tutor", "teach"), Actor::user())
        .expect("created into Noesis");
    h.wait_for("the busy budget was asked", || !budget.admits().is_empty())
        .await;
    gate.disable();
    h.wait_for("the Noesis agent parked", || agent(&h, tutor.id).paused)
        .await;
    assert_eq!(
        agent(&h, tutor.id).status,
        AgentStatus::Ready,
        "parked, not failed"
    );
    assert!(
        h.client.requests().is_empty(),
        "no model call while disabled"
    );
    assert!(types(h.store.as_ref(), tutor.id).contains(&"agent.paused".to_owned()));

    let (plain, _) = h
        .runtime
        .create_agent(spec("ordinary work"), Actor::user())
        .expect("an ordinary agent is untouched by Noesis");
    h.wait_until(plain.id, AgentStatus::is_terminal).await;
    assert_eq!(h.client.requests().len(), 1, "the ordinary agent ran");
    assert!(agent(&h, tutor.id).paused);
}

#[tokio::test]
async fn every_command_that_gives_work_is_refused_while_noesis_is_disabled() {
    let gate = NoesisGate::new();
    let mut seeded = None;
    let h = noesis_harness(
        vec![],
        &gate,
        |store| {
            let waiting = AgentStatus::Waiting {
                condition: WaitCondition::Inbox,
            };
            let failed = AgentStatus::Failed {
                error: "the model broke".into(),
            };
            seeded = Some((
                seed_noesis(store, "noesis.tutor", waiting),
                seed_noesis(store, "noesis.tutor", failed),
            ));
        },
        |_| {},
    )
    .await;
    let (id, failed) = seeded.expect("seeded");
    let before = h
        .store
        .events(0, 10_000, &EventFilter::default())
        .unwrap()
        .len();
    let (row, failed_row) = (agent(&h, id), agent(&h, failed));
    let refused = |result: Result<(), RuntimeError>, of: AgentId, what: &str| match result {
        Err(RuntimeError::NoesisDisabled(named)) => assert_eq!(named, of, "{what}"),
        other => panic!("{what} was not refused for Noesis: {other:?}"),
    };
    let text = || vec![ContentBlock::text("hello")];
    let send = h.runtime.send_message(id, text(), Actor::user());
    refused(send.map(drop), id, "send_message");
    let reply = h
        .runtime
        .reply_to_question(id, 1, text(), Actor::user(), None);
    refused(reply.map(drop), id, "reply_to_question");
    let wake = h.runtime.wake_agent(failed, Actor::user());
    refused(wake.map(drop), failed, "wake");
    let restart = h.runtime.restart_agent(failed, Actor::user());
    refused(restart.map(drop), failed, "restart");
    refused(
        h.runtime.request_compaction(id, Actor::user()),
        id,
        "compact",
    );
    let model = AgentUpdate {
        model: Some(ModelRef::new("anthropic", "claude-haiku-5-5")),
        ..Default::default()
    };
    let update = h.runtime.update_agent(id, model, Actor::user());
    refused(update.map(drop), id, "model change");
    h.runtime
        .pause_agent(id, false, Actor::user())
        .expect("a pause gives no work");
    let paused = h
        .store
        .events(0, 10_000, &EventFilter::default())
        .unwrap()
        .len();
    let resume = h.runtime.resume_agent(id, false, Actor::user());
    refused(resume.map(drop), id, "resume");
    let resume = h.runtime.resume_agent(id, true, Actor::user());
    refused(resume.map(drop), id, "subtree resume");

    let after = h
        .store
        .events(0, 10_000, &EventFilter::default())
        .unwrap()
        .len();
    assert_eq!(after, paused, "no refused command wrote anything");
    assert_eq!(paused, before + 1, "only the pause committed");
    let mut expected = row.clone();
    let now_row = agent(&h, id);
    expected.paused = true;
    expected.updated_at = now_row.updated_at;
    assert_eq!(now_row, expected, "the agent changed only by the pause");
    assert_eq!(
        agent(&h, failed),
        failed_row,
        "the failed agent is untouched"
    );
    assert!(
        h.store.inbox(id, 0).unwrap().is_empty(),
        "no message was queued"
    );
    assert!(h.client.requests().is_empty());
}

#[tokio::test]
async fn an_admitted_turn_is_charged_exactly_its_own_usage() {
    let gate = NoesisGate::new();
    let budget = Recording::new(Answer::Admit);
    gate.enable(budget.clone());
    let h = noesis_harness(
        vec![
            ScriptedClient::text("first answer"),
            ScriptedClient::tool_call("complete_task", json!({"summary": "done"})),
        ],
        &gate,
        |_| {},
        |_| {},
    )
    .await;
    let (tutor, _) = h
        .runtime
        .create_agent(noesis_spec("noesis.tutor", "teach"), Actor::user())
        .expect("created into Noesis");
    // A text-only reply is nudged on: two turns, each one model call.
    h.wait_until(tutor.id, AgentStatus::is_terminal).await;
    h.wait_for("both charges settled", || budget.settled().len() == 2)
        .await;
    let total = agent(&h, tutor.id).stats.usage;
    assert_eq!(
        budget.admits(),
        vec![(tutor.id, NoesisLane::Interactive); 2],
        "one admit per turn"
    );
    let settled = budget.settled();
    assert!(settled.iter().all(|(id, _)| *id == tutor.id));
    let spent: Vec<(u64, u64)> = settled
        .iter()
        .map(|(_, u)| (u.input_tokens, u.output_tokens))
        .collect();
    // The scripted usage: 10 in each; out is len/4 + 1 for text, 8 + 1 per call.
    assert_eq!(
        spent,
        vec![(10, 4), (10, 9)],
        "each turn is charged its own call"
    );
    assert_eq!(total.input_tokens, 20);
    assert_eq!(total.output_tokens, 13);
}

#[tokio::test]
async fn a_busy_budget_holds_the_turn_until_it_admits() {
    let gate = NoesisGate::new();
    let budget = Recording::new(Answer::BusyThenAdmit(1));
    gate.enable(budget.clone());
    let h = noesis_harness(
        vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "done"}),
        )],
        &gate,
        |_| {},
        |_| {},
    )
    .await;
    let (tutor, _) = h
        .runtime
        .create_agent(noesis_spec("noesis.tutor", "teach"), Actor::user())
        .expect("created");
    h.wait_until(tutor.id, AgentStatus::is_terminal).await;
    assert!(!agent(&h, tutor.id).paused, "busy holds, it does not park");
    assert_eq!(budget.admits().len(), 2, "refused once, then admitted");
    assert_eq!(budget.settled().len(), 1, "only the admitted turn settles");
    assert_eq!(h.client.requests().len(), 1);
}

#[tokio::test]
async fn an_exhausted_budget_parks_the_agent_without_a_model_call() {
    let gate = NoesisGate::new();
    let budget = Recording::new(Answer::Exhausted);
    gate.enable(budget.clone());
    let h = noesis_harness(vec![ScriptedClient::text("unused")], &gate, |_| {}, |_| {}).await;
    let (tutor, _) = h
        .runtime
        .create_agent(noesis_spec("noesis.tutor", "teach"), Actor::user())
        .expect("created");
    h.wait_for("the exhausted agent parked", || agent(&h, tutor.id).paused)
        .await;
    assert_eq!(agent(&h, tutor.id).status, AgentStatus::Ready);
    assert_eq!(budget.admits().len(), 1);
    assert!(
        budget.settled().is_empty(),
        "nothing was admitted to settle"
    );
    assert!(h.client.requests().is_empty());
}

/// An agent of `role` admitted just as the gate drains, before its model call.
async fn drained_at_admission(
    role: &str,
    responses: Vec<aether_model::testing::ScriptedResponse>,
) -> (Harness, AgentId, Arc<Recording>) {
    let gate = NoesisGate::new();
    let budget = Recording::draining(&gate);
    gate.enable(budget.clone());
    let h = noesis_harness(responses, &gate, |_| {}, |_| {}).await;
    let (agent, _) = h
        .runtime
        .create_agent(noesis_spec(role, "work"), Actor::user())
        .expect("created");
    h.wait_for("the charge settled", || budget.settled().len() == 1)
        .await;
    h.wait_until(agent.id, |s| s.kind() != AgentStatusKind::Running)
        .await;
    (h, agent.id, budget)
}

#[tokio::test]
async fn a_draining_gate_stops_background_work_at_its_model_call() {
    let done = ScriptedClient::tool_call("complete_task", json!({"summary": "done"}));
    let (h, id, budget) = drained_at_admission("noesis.extractor", vec![done]).await;
    assert_eq!(
        agent(&h, id).status,
        AgentStatus::Waiting {
            condition: WaitCondition::Inbox
        },
        "interrupted, not failed"
    );
    assert!(types(h.store.as_ref(), id).contains(&"agent.interrupted".to_owned()));
    assert!(
        h.client.requests().is_empty(),
        "no model call after the drain"
    );
    assert_eq!(budget.admits(), vec![(id, NoesisLane::Background)]);
    assert_eq!(budget.settled()[0].1.input_tokens, 0, "nothing was spent");
}

#[tokio::test]
async fn a_draining_gate_lets_an_interactive_turn_finish() {
    let done = ScriptedClient::tool_call("complete_task", json!({"summary": "done"}));
    let (h, id, budget) = drained_at_admission("noesis.tutor", vec![done]).await;
    let status = h.wait_until(id, AgentStatus::is_terminal).await;
    assert!(
        matches!(status, AgentStatus::Completed { .. }),
        "the interactive turn ran on: {status:?}"
    );
    assert_eq!(h.client.requests().len(), 1);
    assert_eq!(budget.admits(), vec![(id, NoesisLane::Interactive)]);
}

#[tokio::test]
async fn cancelling_noesis_turns_interrupts_them_and_keeps_the_agents() {
    let gate = NoesisGate::new();
    let budget = Recording::new(Answer::Admit);
    gate.enable(budget.clone());
    let h = noesis_harness(vec![ScriptedClient::text("held")], &gate, |_| {}, |_| {}).await;
    let _release = h.client.hold_next_response();
    let (tutor, _) = h
        .runtime
        .create_agent(noesis_spec("noesis.tutor", "teach"), Actor::user())
        .expect("created");
    h.wait_for("the held model call", || h.client.requests().len() == 1)
        .await;
    assert_eq!(
        h.runtime.noesis_turns_running(),
        vec![(tutor.id, NoesisLane::Interactive)]
    );
    assert_eq!(
        h.runtime
            .cancel_noesis_turns(Some(NoesisLane::Background))
            .await
            .unwrap(),
        0
    );
    let cancelled = h.runtime.cancel_noesis_turns(None).await.expect("cancel");
    assert_eq!(cancelled, 1);
    assert!(h.runtime.noesis_turns_running().is_empty());
    let status = h
        .wait_until(tutor.id, |s| s.kind() != AgentStatusKind::Running)
        .await;
    assert!(!status.is_terminal(), "interrupted, not failed: {status:?}");
    h.wait_for("the charge settled", || budget.settled().len() == 1)
        .await;
}

#[tokio::test]
async fn recovery_parks_a_noesis_agent_it_would_requeue_while_disabled() {
    let gate = NoesisGate::new();
    let mut seeded = None;
    let h = noesis_harness(
        vec![ScriptedClient::text("must not be asked")],
        &gate,
        |store| seeded = Some(seed_noesis(store, "noesis.tutor", AgentStatus::Running)),
        |_| {},
    )
    .await;
    let id = seeded.expect("seeded");
    h.wait_for("recovered", || agent(&h, id).status != AgentStatus::Running)
        .await;
    let row = agent(&h, id);
    assert!(row.paused, "parked by recovery: {:?}", row.status);
    assert_eq!(row.status, AgentStatus::Ready);
    assert!(h.client.requests().is_empty());
}

#[tokio::test]
async fn only_an_enabled_gate_creates_into_noesis_and_reserved_roles_need_it() {
    let gate = NoesisGate::new();
    let h = noesis_harness(vec![], &gate, |_| {}, |_| {}).await;
    let invalid = |s: CreateAgentSpec, what: &str| match h.runtime.create_agent(s, Actor::user()) {
        Err(RuntimeError::Invalid(_)) => {}
        other => panic!("{what} was not refused: {other:?}"),
    };
    let mut reserved = spec("pose as noesis");
    reserved.role = "noesis.tutor".into();
    invalid(
        noesis_spec("noesis.tutor", "teach"),
        "creation into a disabled Noesis",
    );
    invalid(reserved.clone(), "a reserved role outside Noesis");

    gate.enable(Recording::new(Answer::Exhausted));
    invalid(reserved, "a reserved role outside Noesis, enabled");
    let (tutor, _) = h
        .runtime
        .create_agent(noesis_spec("noesis.tutor", "teach"), Actor::user())
        .expect("enabled: created into Noesis");
    assert_eq!(
        h.store.agent_subsystem(tutor.id).unwrap().as_deref(),
        Some(NOESIS_SUBSYSTEM)
    );
    invalid(
        noesis_spec("worker", "spend unseen"),
        "a Noesis agent without a noesis.* role",
    );
    let mut child = spec("escape the gate");
    child.parent = Some(tutor.id);
    invalid(child, "a child outside Noesis under a Noesis parent");
    let mut plain = spec("ordinary");
    plain.start = false;
    let (plain, _) = h
        .runtime
        .create_agent(plain, Actor::user())
        .expect("ordinary");
    assert_eq!(h.store.agent_subsystem(plain.id).unwrap(), None);
}

#[tokio::test]
async fn a_noesis_agents_child_belongs_to_noesis() {
    let gate = NoesisGate::new();
    let budget = Recording::new(Answer::Admit);
    gate.enable(budget.clone());
    let delegate = json!({"role":"noesis.helper", "objective":"check", "model":"anthropic/claude-sonnet-5-5", "effort":"low", "wait":true});
    let h = noesis_harness(
        vec![
            ScriptedClient::tool_call("delegate", delegate),
            ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
            ScriptedClient::tool_call("complete_task", json!({"summary":"done"})),
        ],
        &gate,
        |_| {},
        |_| {},
    )
    .await;
    let (tutor, _) = h
        .runtime
        .create_agent(noesis_spec("noesis.tutor", "delegate"), Actor::user())
        .expect("created");
    h.wait_until(tutor.id, AgentStatus::is_terminal).await;
    let children = h
        .store
        .agents(&AgentFilter {
            parent: Some(tutor.id),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(
        h.store.agent_subsystem(children[0].id).unwrap().as_deref(),
        Some(NOESIS_SUBSYSTEM)
    );
    assert!(
        budget.admits().iter().any(|(id, _)| *id == children[0].id),
        "the child's turns are admitted by the Noesis budget"
    );
}

#[tokio::test]
async fn a_noesis_agent_cannot_delegate_once_the_gate_drains() {
    let delegate = json!({"role":"noesis.helper", "objective":"check", "model":"anthropic/claude-sonnet-5-5", "effort":"low"});
    let (h, id, _) = drained_at_admission(
        "noesis.tutor",
        vec![ScriptedClient::tool_call("delegate", delegate)],
    )
    .await;
    h.wait_for("parked before its next turn", || agent(&h, id).paused)
        .await;
    let children = h
        .store
        .agents(&AgentFilter {
            parent: Some(id),
            ..Default::default()
        })
        .unwrap();
    assert!(children.is_empty(), "the delegation was refused");
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .unwrap();
    let outcome = agent_support::settled_outcome(&events, "delegate");
    assert!(outcome.is_error, "refused as a tool error: {outcome:?}");
    assert_eq!(
        h.client.requests().len(),
        1,
        "the drained gate starts no next turn"
    );
}
