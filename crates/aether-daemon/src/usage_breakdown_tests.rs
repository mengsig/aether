use std::time::Duration;

use aether_core::agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, InvocationId, RunId, SnapshotId};
use aether_core::model::{ModelRef, Usage};
use aether_core::time::now;
use aether_protocol::read_models::{UsageGroup, UsageScope};
use aether_store::{Commit, Mutation, SqliteStore, Store, StoreError};

use super::{UsageRequest, usage_breakdown};

fn usage(input_tokens: u64, cost_usd: f64) -> Usage {
    Usage {
        input_tokens,
        cost_usd,
        ..Usage::default()
    }
}

fn complete(store: &SqliteStore, agent: AgentId, run: RunId, model: &ModelRef, spent: Usage) {
    let kind = EventKind::InvocationCompleted {
        invocation: InvocationId::new(),
        snapshot: SnapshotId::new(),
        model: model.clone(),
        usage: spent,
        stop_reason: None,
        duration_ms: 1,
    };
    let draft = EventDraft::new(Actor::System, kind).agent(agent).run(run);
    store.commit(Commit::new().event(draft)).expect("commit");
}

fn request(scope: UsageScope, by: UsageGroup) -> UsageRequest {
    UsageRequest {
        scope,
        by,
        since: None,
        until: None,
        limit: 1000,
    }
}

/// Three agents in two runs on two models; costs are exact in binary, so
/// the sums compare equal.
fn fleet() -> (SqliteStore, [AgentId; 3], [RunId; 2], Usage) {
    let store = SqliteStore::open_in_memory().expect("open");
    let agents = [AgentId::new(), AgentId::new(), AgentId::new()];
    let runs = [RunId::new(), RunId::new()];
    let opus = ModelRef::new("anthropic", "opus");
    let mini = ModelRef::new("openai", "mini");
    let spends = [
        (agents[0], runs[0], &opus, usage(100, 0.5)),
        (agents[1], runs[0], &mini, usage(10, 0.25)),
        (agents[0], runs[0], &opus, usage(200, 1.0)),
        (agents[2], runs[1], &mini, usage(1, 0.125)),
    ];
    let mut total = Usage::default();
    for (agent, run, model, spent) in spends {
        complete(&store, agent, run, model, spent);
        total += spent;
    }
    (store, agents, runs, total)
}

#[test]
fn the_total_is_the_sum_of_every_completed_invocation_and_rows_split_it() {
    let (store, _, _, total) = fleet();
    let breakdown = usage_breakdown(&store, &request(UsageScope::default(), UsageGroup::Model))
        .expect("breakdown");
    assert_eq!(breakdown.total, total);
    let keys: Vec<_> = breakdown.rows.iter().map(|row| row.key.as_str()).collect();
    assert_eq!(keys, ["anthropic/opus", "openai/mini"]);
    assert_eq!(breakdown.rows[0].usage.input_tokens, 300);
    assert_eq!(breakdown.rows[0].invocations, 2);
    assert_eq!(breakdown.rows[1].usage.input_tokens, 11);
    let mut summed = Usage::default();
    for row in &breakdown.rows {
        summed += row.usage;
    }
    assert_eq!(summed, total);
    assert!(!breakdown.truncated);
}

#[test]
fn the_same_log_always_answers_with_the_same_rows() {
    let (store, _, _, _) = fleet();
    for by in [
        UsageGroup::Model,
        UsageGroup::Provider,
        UsageGroup::Run,
        UsageGroup::Day,
    ] {
        let first = usage_breakdown(&store, &request(UsageScope::default(), by)).expect("first");
        let again = usage_breakdown(&store, &request(UsageScope::default(), by)).expect("again");
        assert_eq!(first, again, "{by:?}");
        assert!(first.rows.windows(2).all(|pair| pair[0].key < pair[1].key));
    }
}

#[test]
fn a_scope_narrows_the_total_and_a_limit_truncates_only_the_rows() {
    let (store, agents, runs, total) = fleet();
    let by_run = UsageScope {
        run: Some(runs[1]),
        ..UsageScope::default()
    };
    let breakdown = usage_breakdown(&store, &request(by_run, UsageGroup::Provider)).expect("run");
    assert_eq!(breakdown.total.input_tokens, 1);
    let by_agent = UsageScope {
        agent: Some(agents[0]),
        ..UsageScope::default()
    };
    let breakdown = usage_breakdown(&store, &request(by_agent, UsageGroup::Run)).expect("agent");
    assert_eq!(breakdown.total.input_tokens, 300);
    assert_eq!(breakdown.rows.len(), 1);
    assert_eq!(breakdown.rows[0].key, runs[0].to_string());

    let mut limited = request(UsageScope::default(), UsageGroup::Run);
    limited.limit = 1;
    let breakdown = usage_breakdown(&store, &limited).expect("limited");
    assert_eq!(breakdown.rows.len(), 1);
    assert!(breakdown.truncated);
    assert_eq!(breakdown.total, total, "the total counts the rows cut too");
}

#[test]
fn a_window_takes_since_inclusive_and_until_exclusive() {
    let store = SqliteStore::open_in_memory().expect("open");
    let (agent, run) = (AgentId::new(), RunId::new());
    let model = ModelRef::new("anthropic", "opus");
    complete(&store, agent, run, &model, usage(1, 0.0));
    std::thread::sleep(Duration::from_millis(5));
    let middle = now();
    std::thread::sleep(Duration::from_millis(5));
    complete(&store, agent, run, &model, usage(10, 0.0));

    let mut later = request(UsageScope::default(), UsageGroup::Model);
    later.since = Some(middle);
    assert_eq!(
        usage_breakdown(&store, &later)
            .expect("since")
            .total
            .input_tokens,
        10
    );
    let mut earlier = request(UsageScope::default(), UsageGroup::Model);
    earlier.until = Some(middle);
    assert_eq!(
        usage_breakdown(&store, &earlier)
            .expect("until")
            .total
            .input_tokens,
        1
    );
}

fn agent(parent: Option<AgentId>, run: RunId, role: &str) -> Agent {
    let at = now();
    Agent {
        id: AgentId::new(),
        parent,
        role: Role::new(role),
        name: role.into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "m"),
        fallbacks: Vec::new(),
        status: AgentStatus::Created,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        archived: false,
        paused: false,
        paused_via: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        engine_alive: false,
        last_event_at: None,
        config: AgentConfig::default(),
        stats: AgentStats::default(),
        run,
        created_at: at,
        updated_at: at,
    }
}

fn create(store: &SqliteStore, agent: &Agent) {
    let created = Mutation::CreateAgent {
        agent: agent.clone(),
        subsystem: None,
    };
    store.commit(Commit::new().mutate(created)).expect("create");
}

/// Retention removed another run's invocations: a subtree or role scope
/// still answers, while a fleet-wide one says the history is gone.
#[test]
fn history_expired_elsewhere_fails_only_a_scope_that_reads_it() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("aether.db");
    let store = SqliteStore::open(&path).expect("open");
    let model = ModelRef::new("anthropic", "opus");
    let (gone_run, kept_run) = (RunId::new(), RunId::new());
    let gone = agent(None, gone_run, "reviewer");
    let lead = agent(None, kept_run, "lead");
    let worker = agent(Some(lead.id), kept_run, "worker");
    for (agent, spent) in [(&gone, 1000), (&lead, 100), (&worker, 10)] {
        create(&store, agent);
        complete(&store, agent.id, agent.run, &model, usage(spent, 0.0));
    }
    let conn = rusqlite::Connection::open(&path).expect("expire");
    conn.execute_batch(&format!(
        "INSERT INTO expired_event_spans SELECT run, COALESCE(agent_id,''), COALESCE(parent_agent_id,''), COALESCE(task_id,''), type, family, MIN(seq), MAX(seq) FROM events WHERE run = '{gone_run}' GROUP BY run, agent_id, parent_agent_id, task_id, type, family;
         DELETE FROM events WHERE run = '{gone_run}';"
    ))
    .expect("expire the other run");
    drop(conn);

    let subtree = UsageScope {
        subtree: Some(lead.id),
        ..UsageScope::default()
    };
    let breakdown = usage_breakdown(&store, &request(subtree, UsageGroup::Role)).expect("subtree");
    assert_eq!(breakdown.total.input_tokens, 110);
    let keys: Vec<_> = breakdown.rows.iter().map(|row| row.key.as_str()).collect();
    assert_eq!(keys, ["lead", "worker"]);
    let leads = UsageScope {
        role_prefix: Some("lead".into()),
        ..UsageScope::default()
    };
    let breakdown = usage_breakdown(&store, &request(leads, UsageGroup::Run)).expect("role");
    assert_eq!(breakdown.total.input_tokens, 100);

    let fleet = usage_breakdown(&store, &request(UsageScope::default(), UsageGroup::Model));
    assert!(
        matches!(fleet, Err(StoreError::HistoryExpired { .. })),
        "{fleet:?}"
    );
}
