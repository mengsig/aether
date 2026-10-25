use super::*;
use crate::RunningTurn;
use crate::commands::CreateAgentSpec;
use aether_core::agent::AgentStatus;
use aether_store::{Commit, EventFilter, Mutation};
use tokio_util::sync::CancellationToken;

const IDLE: Duration = Duration::from_secs(10 * 60);

async fn quiet_runtime() -> (Arc<Runtime>, tempfile::TempDir) {
    let (runtime, config_dir) = crate::test_runtime(|_| {}).await;
    // The tests set scheduler state by hand; the loop would race them.
    runtime.shutdown.cancel();
    (runtime, config_dir)
}

fn worker(runtime: &Runtime) -> Agent {
    let (agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("agent");
    agent
}

/// The stuck state: marked running, no lease, no turn, nothing to wake it.
fn strand(runtime: &Runtime, mut agent: Agent) -> Agent {
    agent.status = AgentStatus::Running;
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
        .expect("strand");
    let stored = runtime.store.agent(agent.id).expect("read").expect("agent");
    assert_eq!(stored.status, AgentStatus::Running);
    assert!(runtime.store.lease(agent.id).expect("lease").is_none());
    stored
}

fn hold_turn(runtime: &Runtime, agent: AgentId) {
    runtime.sched.lock().running.insert(
        agent,
        RunningTurn {
            task: None,
            cancel: CancellationToken::new(),
            cancel_request: None,
            stop_reason: None,
            interrupt: CancellationToken::new(),
            addressed: true,
            provider: "anthropic".into(),
            engine: false,
            on_provider: true,
        },
    );
}

fn recovered_reasons(runtime: &Runtime, agent: AgentId) -> Vec<serde_json::Value> {
    let filter = EventFilter {
        agent: Some(agent),
        ..Default::default()
    };
    runtime
        .store
        .events(0, 1000, &filter)
        .expect("events")
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::Custom { name, payload } if name == AGENT_RECOVERED => {
                Some(payload["reason"].clone())
            }
            _ => None,
        })
        .collect()
}

fn minutes(n: i64) -> chrono::Duration {
    chrono::Duration::minutes(n)
}

#[tokio::test]
async fn only_a_silent_running_agent_with_no_turn_is_an_orphan() {
    let (runtime, _dir) = quiet_runtime().await;
    let mut agent = worker(&runtime);
    let at = agent.updated_at + minutes(11);
    assert!(!is_orphan(&agent, false, None, at, IDLE), "a ready agent");
    agent.status = AgentStatus::Running;
    assert!(is_orphan(&agent, false, None, at, IDLE));
    assert!(!is_orphan(&agent, true, None, at, IDLE), "a turn in flight");
    let early = agent.updated_at + minutes(9);
    assert!(!is_orphan(&agent, false, None, early, IDLE), "not idle yet");
    let recent = Some(agent.updated_at + minutes(5));
    assert!(
        !is_orphan(&agent, false, recent, at, IDLE),
        "a recent event resets the silence"
    );
    let stale = Some(agent.updated_at - minutes(30));
    assert!(
        is_orphan(&agent, false, stale, at, IDLE),
        "an old event does not hide a later update"
    );
}

#[test]
fn the_sweep_runs_on_its_interval_and_never_when_off() {
    let start = now();
    let interval = Duration::from_secs(60);
    assert!(sweep_due(None, start, interval), "the first pass sweeps");
    let soon = start + chrono::Duration::seconds(59);
    assert!(!sweep_due(Some(start), soon, interval));
    let later = start + chrono::Duration::seconds(60);
    assert!(sweep_due(Some(start), later, interval));
    assert!(!sweep_due(None, later, Duration::ZERO), "zero turns it off");
    assert!(
        !sweep_due(Some(later), start, interval),
        "a clock that went back does not sweep"
    );
}

#[tokio::test]
async fn a_stranded_running_agent_is_recovered_with_a_reason() {
    let (runtime, _dir) = quiet_runtime().await;
    let agent = strand(&runtime, worker(&runtime));
    let at = now() + minutes(11);

    let recovered = sweep(&runtime, at, IDLE).expect("sweep");

    assert_eq!(recovered, vec![agent.id]);
    let after = runtime.store.agent(agent.id).expect("read").expect("agent");
    assert_ne!(
        after.status,
        AgentStatus::Running,
        "left running: {after:?}"
    );
    assert!(runtime.store.lease(agent.id).expect("lease").is_none());
    assert_eq!(
        recovered_reasons(&runtime, agent.id),
        vec![serde_json::json!(REASON_NO_TURN)]
    );
    assert!(
        sweep(&runtime, at, IDLE).expect("again").is_empty(),
        "a recovered agent is not recovered twice"
    );
}

#[tokio::test]
async fn the_sweep_leaves_turns_in_flight_and_recent_agents_alone() {
    let (runtime, _dir) = quiet_runtime().await;
    let busy = strand(&runtime, worker(&runtime));
    hold_turn(&runtime, busy.id);
    let fresh = strand(&runtime, worker(&runtime));

    let in_flight = sweep(&runtime, now() + minutes(11), IDLE).expect("sweep");
    assert_eq!(in_flight, vec![fresh.id], "only the agent with no turn");
    let fresher = strand(&runtime, worker(&runtime));
    let early = sweep(&runtime, now() + minutes(1), IDLE).expect("sweep");

    assert!(early.is_empty(), "recovered before it went idle: {early:?}");
    for id in [busy.id, fresher.id] {
        let agent = runtime.store.agent(id).expect("read").expect("agent");
        assert_eq!(agent.status, AgentStatus::Running);
        assert!(recovered_reasons(&runtime, id).is_empty());
    }
}

#[tokio::test]
async fn sweep_if_due_respects_the_configured_interval() {
    let (runtime, _dir) = crate::test_runtime(|c| {
        c.orphan_sweep_interval = Duration::from_secs(3600);
        c.orphan_idle = Duration::ZERO;
    })
    .await;
    runtime.shutdown.cancel();
    runtime.sched.lock().last_orphan_sweep = None;
    let first = strand(&runtime, worker(&runtime));

    sweep_if_due(&runtime).expect("first sweep");
    let swept_at = runtime.sched.lock().last_orphan_sweep;
    let second = strand(&runtime, worker(&runtime));
    sweep_if_due(&runtime).expect("second sweep");

    assert!(swept_at.is_some(), "the first pass records its time");
    assert_eq!(runtime.sched.lock().last_orphan_sweep, swept_at);
    assert_eq!(recovered_reasons(&runtime, first.id).len(), 1);
    assert!(
        recovered_reasons(&runtime, second.id).is_empty(),
        "swept again before the interval passed"
    );
}

#[tokio::test]
async fn replies_report_whether_a_turn_runs_and_the_last_event() {
    let (runtime, _dir) = quiet_runtime().await;
    let idle = worker(&runtime);
    let busy = worker(&runtime);
    hold_turn(&runtime, busy.id);
    let mut agents = vec![idle.clone(), busy.clone()];

    runtime.annotate_agents(&mut agents).expect("annotate");

    assert!(!agents[0].engine_alive);
    assert!(agents[1].engine_alive);
    let last = runtime.store.last_event_at(idle.id).expect("last event");
    assert!(last.is_some(), "creation records an event");
    assert_eq!(agents[0].last_event_at, last);
    let stored = runtime.store.agent(busy.id).expect("read").expect("agent");
    assert!(!stored.engine_alive, "annotation is reply-only");
}

/// A turn that ends between the sweep's listing and its decision: the listing
/// still says running, but the decision must see the end the turn recorded.
#[tokio::test]
async fn a_turn_that_finishes_after_the_listing_is_not_recovered() {
    let (runtime, _dir) = quiet_runtime().await;
    let agent = strand(&runtime, worker(&runtime));
    hold_turn(&runtime, agent.id);
    let listed = runtime
        .store
        .agents(&AgentFilter {
            statuses: vec![AgentStatusKind::Running],
            ..Default::default()
        })
        .expect("listing");
    assert_eq!(
        listed.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![agent.id]
    );

    let mut finished = agent.clone();
    finished.status = AgentStatus::Ready;
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(finished)))
        .expect("the turn records its end");
    runtime.sched.lock().running.remove(&agent.id);

    let at = now() + minutes(11);
    for stale in &listed {
        let decided = confirmed_orphan(&runtime, stale.id, at, Duration::ZERO).expect("decide");
        assert!(decided.is_none(), "recovered a finished turn: {decided:?}");
    }
    assert!(
        sweep(&runtime, at, Duration::ZERO)
            .expect("sweep")
            .is_empty()
    );
    let after = runtime.store.agent(agent.id).expect("read").expect("agent");
    assert_eq!(after.status, AgentStatus::Ready);
    assert!(recovered_reasons(&runtime, agent.id).is_empty());
}

/// A write that lands while recovery reads the agent (a pause, a cancel)
/// wins: recovery leaves the agent for the next sweep instead of undoing it.
#[tokio::test]
async fn recovery_leaves_an_agent_written_to_while_it_read() {
    let (runtime, _dir) = quiet_runtime().await;
    let read = strand(&runtime, worker(&runtime));
    let mut paused = read.clone();
    paused.paused = true;
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(paused)))
        .expect("a pause lands meanwhile");
    let lease = Lease::new(read.id, "unknown", read.stats.turns, now(), Duration::ZERO);

    let outcome =
        crate::recovery::recover_agent(&runtime, lease, read.clone(), false, Some(REASON_NO_TURN))
            .expect("recover");

    assert_eq!(outcome, crate::recovery::Outcome::Changed);
    let after = runtime.store.agent(read.id).expect("read").expect("agent");
    assert!(after.paused, "recovery undid the pause: {after:?}");
    assert_eq!(after.status, AgentStatus::Running);
    assert!(recovered_reasons(&runtime, read.id).is_empty());
}
