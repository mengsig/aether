//! An answer on a provider whose sign-in was refused: decided again, the same
//! way, when its commit is rebuilt after contention.
use super::*;
use aether_core::agent::{AgentStatus, BlockReason, ExhaustionClass};

fn ctx_for(agent: Agent) -> TurnCtx {
    TurnCtx {
        agent,
        task: None,
        turn: 0,
        turn_usage: aether_core::model::Usage::default(),
        base_actor: Actor::user(),
        hard_cancel: CancellationToken::new(),
        recovery_checked_for: None,
        user_reply_pending: false,
        reply_given_unrecorded: false,
        start_handoff: None,
        unanswered_tool_error: None,
    }
}

fn create(runtime: &Runtime) -> Agent {
    let spec = CreateAgentSpec {
        role: "worker".into(),
        start: true,
        ..Default::default()
    };
    runtime.create_agent(spec, Actor::user()).expect("create").0
}

/// Suspend `agent` on a refused sign-in an hour from its next probe.
fn suspend_on_credentials(runtime: &Runtime, mut agent: Agent) -> aether_core::time::Timestamp {
    let next_probe = now() + chrono::Duration::hours(1);
    agent.status = AgentStatus::Blocked {
        reason: BlockReason::Exhausted {
            model: agent.model.clone(),
            class: ExhaustionClass::Credentials,
            since: now(),
            reported_reset: None,
            next_probe,
            probes: 0,
            detail: "claude-code failed: Failed to authenticate".into(),
        },
    };
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .expect("suspend");
    crate::scheduler::mark_auth_failed(runtime, &agent.model.provider);
    let bench = format!(
        "{} from the provider",
        ExhaustionClass::Credentials.as_str()
    );
    crate::scheduler::backoff(
        runtime,
        &agent.model.provider,
        bench,
        Duration::from_secs(3600),
    )
    .expect("bench");
    next_probe
}

#[tokio::test]
async fn a_rebuilt_answer_commit_still_releases_the_suspended_agents() {
    let (runtime, _dir) = crate::test_runtime(|_| {}).await;
    let parked = create(&runtime);
    let provider = parked.model.provider.clone();
    let next_probe = suspend_on_credentials(&runtime, parked.clone());
    let mut ctx = ctx_for(create(&runtime));
    let answered = ctx.agent.model.clone();
    assert_eq!(
        answered.provider, provider,
        "both agents run on one provider"
    );

    // The first attempt is built, then lost to a locked database.
    let hold = runtime.hold();
    let mut lost = Commit::new();
    let first = close_exhaustion_episode(&runtime, &mut ctx, &hold, &mut lost, &answered).unwrap();
    drop((hold, lost));
    assert_eq!(first.as_deref(), Some(provider.as_str()));
    assert!(
        runtime.provider_auth(&provider).unwrap().is_some(),
        "nothing settled yet"
    );

    ctx.recovery_checked_for = None;
    let hold = runtime.hold();
    let mut commit = Commit::new();
    let retried =
        close_exhaustion_episode(&runtime, &mut ctx, &hold, &mut commit, &answered).unwrap();
    runtime.apply_held(hold, commit).unwrap();
    let settled = retried.expect("the retry decides the same");
    crate::scheduler::settle_auth_answer(&runtime, &settled);

    let row = runtime.store.agent(parked.id).unwrap().unwrap();
    let AgentStatus::Blocked {
        reason: BlockReason::Exhausted {
            next_probe: due, ..
        },
    } = row.status
    else {
        panic!("still suspended until it probes: {:?}", row.status);
    };
    assert!(due < next_probe, "released to probe now");
    assert!(runtime.provider_auth(&provider).unwrap().is_none());
    assert!(!crate::scheduler::credentials_benched(&runtime, &provider));
    let events = runtime
        .store
        .events(0, 10_000, &aether_store::EventFilter::default())
        .unwrap();
    assert!(events.iter().any(
        |event| matches!(&event.kind, EventKind::ProviderRecovered { provider: p } if *p == provider)
    ));
    let fence = crate::scheduler::auth_answered_event(&provider);
    assert!(
        runtime
            .store
            .latest_custom_event(&[fence.as_str()])
            .unwrap()
            .is_some()
    );
    runtime.shutdown().await;
}
