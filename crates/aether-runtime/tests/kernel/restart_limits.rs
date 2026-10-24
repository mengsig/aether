//! restart limits scenarios and fixtures.
use super::*;

/// The turn limit that stopped an agent is checked at the start of its next
/// turn, so a wake under the same limit would fail again before any model
/// call. Like the deadline: raise it on the failed agent, then wake.
#[tokio::test]
async fn a_wake_at_the_turn_limit_needs_the_limit_raised_first() {
    let h = harness(vec![
        ScriptedClient::text("Hello."),
        ScriptedClient::text("Still here."),
    ])
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let say = |text: &str| {
        h.runtime
            .send_message(agent.id, vec![ContentBlock::text(text)], Actor::user())
            .expect("send")
    };
    // An idle agent parks without a turn; the first message gives it one.
    say("hello");
    h.wait_for("the first turn was taken", || h.client.remaining() == 1)
        .await;
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let limit = |max: u32| AgentUpdate {
        max_turns: Some(max),
        ..Default::default()
    };
    h.runtime
        .update_agent(agent.id, limit(1), Actor::user())
        .expect("cap at the turn already taken");
    say("one more");
    let status = h
        .wait_until(agent.id, |s| s.kind() == AgentStatusKind::Failed)
        .await;
    assert!(
        matches!(&status, AgentStatus::Failed { error } if error.contains("turn limit")),
        "{status:?}"
    );
    assert_eq!(
        h.client.remaining(),
        1,
        "the capped turn made no model call"
    );

    let err = h
        .runtime
        .wake_agent(agent.id, Actor::user())
        .expect_err("the limit still stands");
    assert!(err.to_string().contains("turn limit"), "{err}");

    h.runtime
        .update_agent(agent.id, limit(3), Actor::user())
        .expect("a failed agent's limit can be raised");
    let woken = h.runtime.wake_agent(agent.id, Actor::user()).expect("wake");
    assert_eq!(woken.status.kind(), AgentStatusKind::Ready);
    h.wait_for("the woken agent took its turn", || {
        h.client.remaining() == 0
    })
    .await;
    let status = h
        .wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    assert!(!status.is_terminal(), "{status:?}");
}

/// The deadline that stopped an agent would stop it again on the next tick,
/// so the wake says so instead; moving the deadline is allowed on a failed
/// agent precisely so that the wake can then go through.
#[tokio::test]
async fn a_wake_after_a_deadline_needs_the_deadline_moved_first() {
    let h = harness(vec![ScriptedClient::text("Still here.")]).await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    h.runtime
        .update_agent(
            agent.id,
            AgentUpdate {
                deadline: Some(DeadlineUpdate::Set(now() - chrono::Duration::seconds(1))),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("set a deadline in the past");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Failed)
        .await;

    let err = h
        .runtime
        .wake_agent(agent.id, Actor::user())
        .expect_err("the deadline still stands");
    assert!(err.to_string().contains("deadline"), "{err}");

    h.runtime
        .update_agent(
            agent.id,
            AgentUpdate {
                deadline: Some(DeadlineUpdate::Clear),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("a failed agent's deadline can be moved");
    let woken = h.runtime.wake_agent(agent.id, Actor::user()).expect("wake");
    assert_eq!(woken.status.kind(), AgentStatusKind::Ready);
    // It runs, and stays up: nothing stops it again.
    h.wait_for("the woken agent took its turn", || {
        h.client.remaining() == 0
    })
    .await;
    let status = h
        .wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    assert!(!status.is_terminal(), "{status:?}");
}

/// The copy's provenance says it stands for everything up to the restart, but
/// only the last RESTART_TRANSCRIPT_TAIL messages come across and nothing
/// summarises the rest. The event says how many were left behind, so the gap
/// is visible instead of silent.
#[tokio::test]
async fn a_restart_reports_the_transcript_it_could_not_carry() {
    const SAID: usize = aether_runtime::commands::RESTART_TRANSCRIPT_TAIL + 5;

    let id = AgentId::new();
    let h = harness_with(vec![], |store| {
        let at = now();
        let mut agent = aether_core::agent::Agent {
            archived: false,
            id,
            parent: None,
            role: "worker".into(),
            name: "w".into(),
            scope: Scope::default(),
            model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
            fallbacks: Vec::new(),
            status: AgentStatus::Created,
            current_task: None,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: None,
            paused: false,
            paused_via: None,
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            engine_alive: false,
            last_event_at: None,
            config: AgentConfig::default(),
            stats: Default::default(),
            run: Default::default(),
            created_at: at,
            updated_at: at,
        };
        agent
            .transition(AgentStatus::Cancelled { reason: None }, at)
            .expect("cancelled");
        let mut commit = Commit::new().mutate(Mutation::UpsertAgent(agent));
        for n in 0..SAID {
            commit.push_event(
                EventDraft::new(
                    Actor::System,
                    EventKind::AgentMessage {
                        role: aether_core::model::MessageRole::Assistant,
                        content: vec![ContentBlock::text(format!("step {n}"))],
                        invocation: None,
                    },
                )
                .agent(id),
            );
        }
        store.commit(commit).expect("seed");
    })
    .await;

    h.runtime.restart_agent(id, Actor::user()).expect("restart");
    let restarted = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::AgentRestarted {
                transcript_tail,
                transcript_dropped,
                ..
            } if e.agent_id == Some(id) => Some((transcript_tail, transcript_dropped)),
            _ => None,
        })
        .expect("agent.restarted");
    assert_eq!(
        restarted,
        (aether_runtime::commands::RESTART_TRANSCRIPT_TAIL, 5),
        "the tail carried and the messages left behind"
    );
}

/// "A restart happens once" was a read-then-commit check: two restarts racing
/// on one agent both read `restarted_as == None` and both committed, forking
/// the history the check exists to protect. The decision is made under the
/// commit hold now, so only one of them can see an unrestarted agent. Raced
/// over many agents because the losing window was small, not because it was
/// closed: with the check outside the hold this fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_one_of_several_racing_restarts_creates_a_successor() {
    const RACERS: usize = 4;
    const AGENTS: usize = 40;

    let h = harness(vec![]).await;
    let cancelled: Vec<AgentId> = (0..AGENTS).map(|_| AgentId::new()).collect();
    let at = now();
    let mut seed = Commit::new();
    for id in &cancelled {
        let mut agent = aether_core::agent::Agent {
            archived: false,
            id: *id,
            parent: None,
            role: "worker".into(),
            name: "w".into(),
            scope: Scope::default(),
            model: ModelRef::new("anthropic", "claude-sonnet-5-5"),
            fallbacks: Vec::new(),
            status: AgentStatus::Created,
            current_task: None,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: None,
            paused: false,
            paused_via: None,
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            engine_alive: false,
            last_event_at: None,
            config: AgentConfig::default(),
            stats: Default::default(),
            run: Default::default(),
            created_at: at,
            updated_at: at,
        };
        agent
            .transition(AgentStatus::Cancelled { reason: None }, at)
            .expect("cancelled");
        seed.push_mutation(Mutation::UpsertAgent(agent));
    }
    h.store.commit(seed).expect("seed");

    // Real threads released together on each agent: restart_agent is
    // synchronous and takes the commit lock, so tokio tasks on one worker
    // would only queue behind each other.
    let line = Arc::new(std::sync::Barrier::new(RACERS));
    let cancelled = Arc::new(cancelled);
    let racers: Vec<_> = (0..RACERS)
        .map(|_| {
            let runtime = h.runtime.clone();
            let line = line.clone();
            let ids = cancelled.clone();
            std::thread::spawn(move || {
                ids.iter()
                    .filter(|id| {
                        line.wait();
                        runtime.restart_agent(**id, Actor::user()).is_ok()
                    })
                    .count()
            })
        })
        .collect();
    let won: usize = racers.into_iter().map(|t| t.join().expect("racer")).sum();
    assert_eq!(won, AGENTS, "exactly one restart per agent may win");

    let all = h.store.agents(&AgentFilter::default()).expect("agents");
    for id in cancelled.iter() {
        let successors: Vec<AgentId> = all
            .iter()
            .filter(|a| a.restarted_from == Some(*id))
            .map(|a| a.id)
            .collect();
        assert_eq!(
            successors.len(),
            1,
            "one successor for {id}: {successors:?}"
        );
        assert_eq!(
            h.store.agent(*id).expect("a").expect("e").restarted_as,
            Some(successors[0]),
            "and the old agent points at it"
        );
    }
}
