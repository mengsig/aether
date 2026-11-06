use super::*;
use crate::sqlite::SqliteStore;
use crate::{AgentFilter, Commit, Mutation, Store};
use aether_core::agent::{AgentConfig, AgentStats, AgentStatus, AgentStatusKind, Role, Scope};
use aether_core::id::RunId;
use aether_core::model::ModelRef;

fn agent(run: RunId, usage: Usage) -> Agent {
    let at = aether_core::time::now();
    Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("worker"),
        name: "worker".into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "m"),
        fallbacks: Vec::new(),
        status: AgentStatus::Running,
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
        stats: AgentStats {
            usage,
            ..AgentStats::default()
        },
        run,
        created_at: at,
        updated_at: at,
    }
}

/// The `n`th of a spread of usages over every gap, basis, estimate and
/// reasoning shape. Costs are multiples of 1/64, so sums are exact.
fn shaped(n: usize) -> Usage {
    let gap = [
        None,
        Some(None),
        Some(Some(CostGap::Unpriced)),
        Some(Some(CostGap::Interrupted)),
    ][n % 4];
    let mut usage = Usage {
        input_tokens: (n as u64 * 7) % 50,
        output_tokens: (n as u64 * 3) % 20,
        cache_read_tokens: n as u64 % 5,
        cache_write_tokens: n as u64 % 4,
        cache_write_1h_tokens: n as u64 % 2,
        reasoning_tokens: (n % 3 == 0).then_some(n as u64),
        cost_usd: if n % 5 == 0 {
            0.0
        } else {
            (n % 9) as f64 / 64.0
        },
        cost_complete: gap.is_none(),
        cost_gap: gap.flatten(),
        cost_basis: BASES[n % BASES.len()],
        estimated_cost_usd: (n % 7 == 2).then_some((n % 4) as f64 / 64.0),
    };
    if n % 11 == 0 {
        usage = Usage::default();
    }
    assert!(usage.cache_write_1h_tokens <= usage.cache_write_tokens);
    usage
}

/// What the whole-agent reads say: the fleet without imported agents, and
/// each run with them.
fn assert_totals_match_whole_agents(store: &SqliteStore, runs: &[RunId]) {
    let agents = store.agents(&AgentFilter::default()).unwrap();
    let mut fleet = Usage::default();
    for agent in &agents {
        if agent.status.kind() != AgentStatusKind::Imported {
            fleet += agent.stats.usage;
        }
    }
    assert_eq!(store.fleet_usage().unwrap(), fleet, "the fleet's total");
    for run in runs {
        let members: Vec<&Agent> = agents.iter().filter(|a| a.run == *run).collect();
        let read = store.run_totals(*run).unwrap();
        match aether_core::budget::RunTotals::of(members) {
            None => assert!(read.is_none(), "an emptied run has no totals"),
            Some(expected) => {
                let read = read.expect("a run with members has totals");
                assert_eq!(read.agents, expected.agents, "run {run}'s members");
                assert_eq!(read.usage, expected.usage, "run {run}'s usage");
            }
        }
    }
}

#[test]
fn totals_follow_every_write_as_the_usage_fold_does() {
    let store = SqliteStore::open_in_memory().unwrap();
    let runs = [RunId::new(), RunId::new(), RunId::new()];
    let mut agents: Vec<Agent> = (0..60).map(|n| agent(runs[n % 3], shaped(n))).collect();
    let mut first = Commit::new();
    for agent in &agents {
        first = first.mutate(Mutation::UpsertAgent(agent.clone()));
    }
    store.commit(first).unwrap();
    assert_totals_match_whole_agents(&store, &runs);

    let mut second = Commit::new();
    for (n, agent) in agents.iter_mut().enumerate().filter(|(n, _)| n % 4 == 1) {
        agent.stats.usage = shaped(n * 13 + 5);
        agent.archived = n % 8 == 1;
        if n % 3 == 0 {
            agent.status = AgentStatus::Imported {
                was: AgentStatusKind::Completed,
            };
        }
        if n % 5 == 0 {
            agent.run = runs[(n + 1) % 3];
        }
        second = second.mutate(Mutation::UpsertAgent(agent.clone()));
    }
    store.commit(second).unwrap();
    assert_totals_match_whole_agents(&store, &runs);

    let conn = store.conn.lock();
    conn.execute("DELETE FROM agents WHERE run = ?1", [runs[2].to_string()])
        .unwrap();
    conn.execute(
        "UPDATE agents SET body = '{broken' WHERE id = ?1",
        [agents[0].id.to_string()],
    )
    .unwrap();
    drop(conn);
    assert_totals_match_whole_agents(&store, &runs);
}

#[test]
fn an_emptied_scope_leaves_no_row_behind() {
    let store = SqliteStore::open_in_memory().unwrap();
    let run = RunId::new();
    let only = agent(run, shaped(4));
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(only.clone())))
        .unwrap();
    let conn = store.conn.lock();
    let rows = |conn: &Connection| -> i64 {
        conn.query_row("SELECT COUNT(*) FROM usage_totals", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(rows(&conn), 2, "the run's row and the fleet's");
    conn.execute("DELETE FROM agents WHERE id = ?1", [only.id.to_string()])
        .unwrap();
    assert_eq!(rows(&conn), 0, "no rounding outlives the members");
    assert_eq!(read_totals(&conn, FLEET).unwrap(), (0, Usage::default()));
}

#[test]
fn the_counting_function_answers_only_for_a_whole_agent_under_its_own_id() {
    let whole = agent(RunId::new(), shaped(6));
    let body = serde_json::to_string(&whole).unwrap();
    let id = whole.id.to_string();
    let counted = counted_usage(Some(&id), Some(&body)).expect("a whole agent is counted");
    let expected = serde_json::to_string(&LedgerUsage::of(&whole.stats.usage)).unwrap();
    assert_eq!(counted, expected);
    assert_eq!(counted_usage(Some(&id), Some("{broken")), None);
    assert_eq!(
        counted_usage(Some("agt_e"), Some(r#"{"id":"agt_e"}"#)),
        None
    );
    assert_eq!(
        counted_usage(Some("agt_other"), Some(&body)),
        None,
        "id mismatch"
    );
    assert_eq!(counted_usage(None, Some(&body)), None);
    assert_eq!(counted_usage(Some(&id), None), None);
}

#[test]
fn a_ledger_row_reads_back_as_the_usage_it_was_written_from() {
    let store = SqliteStore::open_in_memory().unwrap();
    let agents: Vec<Agent> = (0..24).map(|n| agent(RunId::new(), shaped(n))).collect();
    let mut commit = Commit::new();
    for agent in &agents {
        commit = commit.mutate(Mutation::UpsertAgent(agent.clone()));
    }
    store.commit(commit).unwrap();
    let ids: Vec<AgentId> = agents
        .iter()
        .map(|a| a.id)
        .chain([AgentId::new()])
        .collect();
    let read = store.agent_usages(&ids).unwrap();
    assert_eq!(read.len(), agents.len(), "an unknown id is left out");
    for agent in &agents {
        let mut expected = agent.stats.usage;
        if !expected.cost_complete {
            expected.cost_gap = Some(expected.cost_gap.unwrap_or(CostGap::Unknown));
        }
        assert_eq!(read[&agent.id], expected, "{}", agent.id);
    }
}

/// One member billing tokens but no reasoning or estimate, with its totals
/// then rewritten past the triggers by `tamper`.
fn store_with_tampered_totals(tamper: &str) -> (SqliteStore, RunId) {
    let store = SqliteStore::open_in_memory().unwrap();
    let run = RunId::new();
    let usage = shaped(4);
    assert!(usage.reasoning_tokens.is_none() && usage.estimated_cost_usd.is_none());
    assert_ne!(usage, Usage::default(), "the member billed");
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(agent(run, usage))))
        .unwrap();
    assert!(store.run_totals(run).unwrap().is_some());
    store.conn.lock().execute(tamper, []).unwrap();
    (store, run)
}

fn is_corrupt<T>(read: &Result<T>) -> bool {
    matches!(read, Err(StoreError::Corrupt(_)))
}

#[test]
fn totals_that_disagree_with_their_members_read_as_a_corrupt_store() {
    let (store, run) = store_with_tampered_totals("UPDATE usage_totals SET members = 0");
    let run_read = store.run_totals(run);
    assert!(
        is_corrupt(&run_read),
        "a first agent, no members: {run_read:?}"
    );
    let fleet_read = store.fleet_usage();
    assert!(
        is_corrupt(&fleet_read),
        "an empty fleet billed: {fleet_read:?}"
    );

    let (store, run) = store_with_tampered_totals("UPDATE usage_totals SET reasoning_members = 2");
    let run_read = store.run_totals(run);
    assert!(
        is_corrupt(&run_read),
        "more reasoners than members: {run_read:?}"
    );
    let fleet_read = store.fleet_usage();
    assert!(
        is_corrupt(&fleet_read),
        "more reasoners than members: {fleet_read:?}"
    );
}
