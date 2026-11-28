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
    let first =
        close_exhaustion_episode(&runtime, &mut ctx, &hold, &mut lost, &answered, now()).unwrap();
    drop((hold, lost));
    assert_eq!(
        first.as_ref().map(|(p, _)| p.as_str()),
        Some(provider.as_str())
    );
    assert!(
        runtime.provider_auth(&provider).unwrap().is_some(),
        "nothing settled yet"
    );

    ctx.recovery_checked_for = None;
    let hold = runtime.hold();
    let mut commit = Commit::new();
    let retried =
        close_exhaustion_episode(&runtime, &mut ctx, &hold, &mut commit, &answered, now()).unwrap();
    runtime.apply_held(hold, commit).unwrap();
    let (settled, at) = retried.expect("the retry decides the same");
    crate::scheduler::settle_auth_answer(&runtime, &settled, at);

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
    let stored = runtime
        .store
        .latest_custom_event(&[fence.as_str()])
        .unwrap()
        .expect("the answer is stored");
    // Expiring a finished run deletes the events naming it; the fence must outlive it.
    assert!(stored.run.is_none(), "the fence belongs to no run");
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_refusal_after_the_answer_survives_its_settlement() {
    let (runtime, _dir) = crate::test_runtime(|_| {}).await;
    let parked = create(&runtime);
    let provider = parked.model.provider.clone();
    suspend_on_credentials(&runtime, parked);
    let answered_at = aether_core::time::now() - chrono::Duration::seconds(1);
    // Another turn is refused after the answer was read, before it settles.
    crate::scheduler::mark_auth_failed(&runtime, &provider);
    crate::scheduler::settle_auth_answer(&runtime, &provider, answered_at);
    assert!(
        crate::scheduler::auth_marked(&runtime, &provider),
        "the newer refusal stands"
    );
    assert!(runtime.provider_auth(&provider).unwrap().is_some());

    crate::scheduler::settle_auth_answer(&runtime, &provider, aether_core::time::now());
    assert!(!crate::scheduler::auth_marked(&runtime, &provider));
    assert!(
        runtime.provider_auth(&provider).unwrap().is_none(),
        "an answer after every refusal clears it"
    );
    runtime.shutdown().await;
}

/// Suspend `agent` on a refused sign-in as a turn does: planned against the
/// episode it may continue, recorded with its events. Returns the row.
fn suspend_through_plan(runtime: &Runtime, agent: &Agent) -> Agent {
    let cause = crate::exhaustion::Exhausted {
        model: agent.model.clone(),
        class: ExhaustionClass::Credentials,
        detail: "claude-code failed: Failed to authenticate".into(),
        retry_after: None,
    };
    let suspension = crate::exhaustion::plan(runtime, agent, &cause).unwrap();
    let mut row = agent.clone();
    row.status = AgentStatus::Blocked {
        reason: suspension.reason.clone(),
    };
    let hold = runtime.hold();
    let mut commit = Commit::new();
    crate::exhaustion::record(
        runtime,
        &hold,
        &mut commit,
        &row,
        &cause,
        &suspension,
        &Actor::Scheduler,
    )
    .unwrap();
    commit.push_mutation(Mutation::UpsertAgent(row.clone()));
    runtime.apply_held(hold, commit).unwrap();
    crate::scheduler::mark_auth_failed(runtime, &agent.model.provider);
    row
}

fn since_of(row: &Agent) -> aether_core::time::Timestamp {
    let AgentStatus::Blocked {
        reason: BlockReason::Exhausted { since, .. },
    } = &row.status
    else {
        panic!("suspended: {:?}", row.status);
    };
    *since
}

/// Record and settle an answer on `answering`'s provider read at `at`.
fn answer(runtime: &Runtime, answering: Agent, at: aether_core::time::Timestamp) {
    let mut ctx = ctx_for(answering);
    let model = ctx.agent.model.clone();
    let hold = runtime.hold();
    let mut commit = Commit::new();
    let answered =
        close_exhaustion_episode(runtime, &mut ctx, &hold, &mut commit, &model, at).unwrap();
    runtime.apply_held(hold, commit).unwrap();
    let (provider, at) = answered.expect("the provider was refused");
    crate::scheduler::settle_auth_answer(runtime, &provider, at);
}

#[tokio::test]
async fn a_failed_probe_after_the_answer_stays_refused_across_a_restart() {
    let store = Arc::new(aether_store::SqliteStore::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let (runtime, dir) = crate::test_runtime_on(store.clone(), dir, |_| {}).await;
    let parked = suspend_through_plan(&runtime, &create(&runtime));
    let provider = parked.model.provider.clone();
    let first_since = since_of(&parked);
    answer(&runtime, create(&runtime), now());
    assert!(runtime.provider_auth(&provider).unwrap().is_none());

    // Its probe is refused again: the episode goes on, the refusal is new.
    let probed = suspend_through_plan(&runtime, &parked);
    let fence = runtime.auth_answer_fence(&provider).unwrap().unwrap();
    assert!(since_of(&probed) > fence && fence >= first_since);
    assert!(runtime.provider_auth(&provider).unwrap().is_some());
    runtime.shutdown().await;

    let (restarted, _dir) = crate::test_runtime_on(store, dir, |_| {}).await;
    assert!(
        restarted.provider_auth(&provider).unwrap().is_some(),
        "the refusal after the answer is still reported"
    );
    restarted.shutdown().await;
}

#[tokio::test]
async fn the_answer_fence_outlives_its_run_and_a_restart() {
    let store = Arc::new(aether_store::SqliteStore::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let (runtime, dir) = crate::test_runtime_on(store.clone(), dir, |_| {}).await;
    let parked = suspend_through_plan(&runtime, &create(&runtime));
    let provider = parked.model.provider.clone();
    let answering = create(&runtime);
    assert_ne!(answering.run, parked.run, "each root agent has its own run");
    answer(&runtime, answering.clone(), now());

    let mut finished = runtime.store.agent(answering.id).unwrap().unwrap();
    finished.status = AgentStatus::Completed { summary: None };
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(finished)))
        .unwrap();
    let expiry = runtime
        // A cutoff past everything the run wrote, as if its retention had passed.
        .expire_run_history(answering.run, now() + chrono::Duration::minutes(1))
        .unwrap();
    assert!(
        matches!(expiry, aether_store::RunExpiry::Expired { .. }),
        "{expiry:?}"
    );
    assert!(runtime.store.agent(answering.id).unwrap().is_none());
    runtime.shutdown().await;

    let (restarted, _dir) = crate::test_runtime_on(store, dir, |_| {}).await;
    assert!(
        restarted.store.agent(parked.id).unwrap().is_some(),
        "the older suspension is still on record"
    );
    assert!(
        restarted.provider_auth(&provider).unwrap().is_none(),
        "the surviving fence says the provider answered after it"
    );
    restarted.shutdown().await;
}

#[tokio::test]
async fn an_answer_read_before_a_refusal_does_not_clear_it() {
    let (runtime, _dir) = crate::test_runtime(|_| {}).await;
    let parked = suspend_through_plan(&runtime, &create(&runtime));
    let provider = parked.model.provider.clone();
    let mut ctx = ctx_for(create(&runtime));
    let model = ctx.agent.model.clone();
    // The answer is read, then another turn is refused before its commit holds.
    let answered_at = now();
    std::thread::sleep(Duration::from_millis(2));
    crate::scheduler::mark_auth_failed(&runtime, &provider);
    let hold = runtime.hold();
    let mut commit = Commit::new();
    let answered =
        close_exhaustion_episode(&runtime, &mut ctx, &hold, &mut commit, &model, answered_at)
            .unwrap();
    runtime.apply_held(hold, commit).unwrap();
    let (settled, at) = answered.expect("the provider was refused");
    assert_eq!(at, answered_at, "the fence is the time the answer was read");
    crate::scheduler::settle_auth_answer(&runtime, &settled, at);
    assert!(crate::scheduler::auth_marked(&runtime, &provider));
    assert!(runtime.provider_auth(&provider).unwrap().is_some());
    runtime.shutdown().await;
}
