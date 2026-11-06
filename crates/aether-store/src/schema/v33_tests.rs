//! V33: each agent's usage moves beside its row and the fleet's and each
//! run's totals into one row each, backfilled from a 1.2.11 store (V28) and
//! kept by triggers. The backfill must count what the 1.2.11 reader counted.

use super::tests::{database_at, version};
use super::*;
use crate::Store;
use aether_core::agent::{
    Agent, AgentConfig, AgentStats, AgentStatus, AgentStatusKind, Role, Scope,
};
use aether_core::id::{AgentId, RunId};
use aether_core::model::{CostBasis, CostGap, Usage};

/// Costs are multiples of 1/64, so every sum and difference is exact in an
/// f64 and the triggers' running totals must equal the fold to the bit.
fn usage(input_tokens: u64, sixty_fourths: u32) -> Usage {
    Usage {
        input_tokens,
        cost_usd: f64::from(sixty_fourths) / 64.0,
        cost_complete: true,
        ..Default::default()
    }
}

/// Usages that exercise every part of the fold: bases, gaps, reasoning,
/// estimates, cache counters, and a member with no cost figure at all.
fn assorted_usages() -> Vec<Usage> {
    let listed = Usage {
        cost_basis: CostBasis::ListPrice,
        cache_write_tokens: 6,
        cache_write_1h_tokens: 2,
        ..usage(10, 96)
    };
    let reported = Usage {
        cost_basis: CostBasis::ProviderReported,
        reasoning_tokens: Some(4),
        cache_read_tokens: 3,
        ..usage(20, 16)
    };
    let unpriced = Usage {
        cost_complete: false,
        cost_gap: Some(CostGap::Unpriced),
        estimated_cost_usd: Some(0.5),
        ..usage(30, 0)
    };
    let interrupted = Usage {
        cost_complete: false,
        cost_gap: Some(CostGap::Interrupted),
        cost_basis: CostBasis::Mixed,
        ..usage(40, 128)
    };
    let gap_without_name = Usage {
        cost_complete: false,
        cost_gap: None,
        output_tokens: 9,
        ..usage(0, 0)
    };
    let tokens_without_basis = usage(50, 0);
    vec![
        listed,
        reported,
        unpriced,
        interrupted,
        gap_without_name,
        tokens_without_basis,
        Usage::default(),
    ]
}

/// A whole agent as the store writes it, fully deserializable.
fn whole_agent(run: RunId, status: AgentStatus, usage: Usage) -> Agent {
    let at = aether_core::time::now();
    Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("worker"),
        name: "worker".into(),
        scope: Scope::default(),
        model: aether_core::model::ModelRef::new("mock", "m"),
        fallbacks: Vec::new(),
        status,
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

/// An agent row as a 1.2.11 binary wrote it: its columns, then the body.
fn insert_row(conn: &Connection, id: &str, status: &str, run: &str, body: &str) {
    conn.execute(
        "INSERT INTO agents(id, parent, role, status, run, created_at, updated_at, body)
         VALUES (?1, NULL, 'worker', ?2, ?3, '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z', ?4)",
        [id, status, run, body],
    )
    .expect("agent row");
}

fn insert_agent(conn: &Connection, agent: &Agent) {
    let body = serde_json::to_string(agent).expect("an agent serializes");
    let id = agent.id.to_string();
    insert_row(
        conn,
        &id,
        agent.status.kind().as_str(),
        &agent.run.to_string(),
        &body,
    );
}

/// The 1.2.11 `fleet_usage`: every row not imported whose body decodes as an
/// agent with the row's id, folded; any other row it quarantined, unsummed.
fn fleet_usage_as_1_2_11_read_it(conn: &Connection) -> (Usage, usize) {
    let mut statement = conn
        .prepare("SELECT id, body FROM agents WHERE status != 'imported'")
        .unwrap();
    let rows: Vec<(String, String)> = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut total = Usage::default();
    let mut quarantined = 0;
    for (id, body) in rows {
        match serde_json::from_str::<Agent>(&body) {
            Ok(agent) if agent.id.to_string() == id => total += agent.stats.usage,
            _ => quarantined += 1,
        }
    }
    (total, quarantined)
}

/// Rows the 1.2.11 reader quarantined: not JSON, JSON but not an agent, and
/// a whole agent stored under another row's id.
fn insert_unreadable_rows(conn: &Connection, run: RunId) {
    let run = run.to_string();
    insert_row(conn, &AgentId::new().to_string(), "failed", &run, "{broken");
    insert_row(conn, "agt_e", "created", &run, r#"{"id":"agt_e"}"#);
    let elsewhere = whole_agent(RunId::new(), AgentStatus::Running, usage(7, 448));
    let body = serde_json::to_string(&elsewhere).unwrap();
    insert_row(conn, &AgentId::new().to_string(), "running", &run, &body);
}

/// A V28 store holding every usage under every status, half archived, over
/// two runs, beside the rows its reader quarantined.
fn a_1_2_11_store(conn: &Connection, runs: [RunId; 2]) -> Vec<Agent> {
    let statuses = [
        AgentStatus::Running,
        AgentStatus::Completed { summary: None },
        AgentStatus::Imported {
            was: AgentStatusKind::Completed,
        },
        AgentStatus::Created,
    ];
    let mut agents = Vec::new();
    for (n, usage) in assorted_usages().into_iter().enumerate() {
        for (k, status) in statuses.iter().enumerate() {
            let mut agent = whole_agent(runs[(n + k) % 2], status.clone(), usage);
            agent.archived = (n + k) % 3 == 0;
            insert_agent(conn, &agent);
            agents.push(agent);
        }
    }
    insert_unreadable_rows(conn, runs[0]);
    assert_eq!(agents.len(), 28, "seven usages under four statuses");
    agents
}

#[test]
fn v33_counts_exactly_what_the_1_2_11_reader_counted() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let previous = database_at(&path, 28);
    let runs = [RunId::new(), RunId::new()];
    let agents = a_1_2_11_store(&previous, runs);
    let (read_by_1_2_11, quarantined) = fleet_usage_as_1_2_11_read_it(&previous);
    assert_eq!(quarantined, 3, "the three unreadable rows");
    assert!(!read_by_1_2_11.cost_complete, "the fold saw a gap");
    drop(previous);

    let store = crate::SqliteStore::open(&path).expect("a 1.2.11 store opens and migrates");
    assert_eq!(
        store.meta("schema_version").unwrap(),
        Some(SCHEMA_VERSION.to_string())
    );
    assert_eq!(store.fleet_usage().unwrap(), read_by_1_2_11);
    let mut whole = Usage::default();
    for agent in store.agents(&crate::AgentFilter::default()).unwrap() {
        if agent.status.kind() != AgentStatusKind::Imported {
            whole += agent.stats.usage;
        }
    }
    assert_eq!(whole, read_by_1_2_11, "today's whole-agent reader agrees");
    for run in runs {
        let members: Vec<Agent> = agents.iter().filter(|a| a.run == run).cloned().collect();
        let totals = store.run_totals(run).unwrap().expect("a run with agents");
        let expected = aether_core::budget::RunTotals::of(&members).unwrap();
        assert_eq!(totals.agents, expected.agents, "imported members count");
        assert_eq!(totals.usage, expected.usage);
    }
    drop(store);
    assert_the_previous_binary_refuses(&path, directory.path());
}

fn assert_the_previous_binary_refuses(path: &std::path::Path, directory: &std::path::Path) {
    let backups: Vec<String> = std::fs::read_dir(directory.join("migration-backups"))
        .expect("a backup was taken")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        backups
            .iter()
            .any(|name| name.starts_with("schema-28-before-33-")),
        "{backups:?}"
    );
    let conn = Connection::open(path).unwrap();
    assert_eq!(version(&conn), SCHEMA_VERSION);
    assert!(
        matches!(
            refuse_newer_store(current_version(&conn).unwrap(), 32),
            Err(StoreError::SchemaTooNew {
                found: 33,
                supported: 32
            })
        ),
        "the previous binary refuses the migrated store and restores the backup"
    );
}

#[test]
fn v33_copies_the_archived_flag_out_of_each_readable_body() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let previous = database_at(&path, 28);
    let agents = a_1_2_11_store(&previous, [RunId::new(), RunId::new()]);
    drop(previous);
    drop(crate::SqliteStore::open(&path).expect("migrates"));

    let conn = Connection::open(&path).unwrap();
    let archived_column = |id: AgentId| -> bool {
        conn.query_row(
            "SELECT archived FROM agents WHERE id = ?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert!(agents.iter().any(|a| a.archived) && agents.iter().any(|a| !a.archived));
    for agent in &agents {
        assert_eq!(archived_column(agent.id), agent.archived, "{}", agent.id);
    }
    let unreadable: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agents WHERE archived = 1 AND NOT json_valid(body)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unreadable, 0, "a body that is not JSON stays unarchived");
}

#[test]
fn v33_replays_over_a_rewound_stamp_without_counting_an_agent_twice() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let conn = database_at(&path, SCHEMA_VERSION);
    let run = RunId::new();
    let agents: Vec<Agent> = assorted_usages()
        .into_iter()
        .map(|u| whole_agent(run, AgentStatus::Running, u))
        .collect();
    for agent in &agents {
        insert_agent(&conn, agent);
    }
    conn.execute(
        "UPDATE meta SET value = '32' WHERE key = 'schema_version'",
        [],
    )
    .unwrap();
    migrate(&conn).expect("the replay");
    assert_eq!(version(&conn), SCHEMA_VERSION);
    let (members, total) =
        crate::sqlite::usage_totals::read_totals(&conn, crate::sqlite::usage_totals::FLEET)
            .unwrap();
    assert_eq!(members, agents.len() as u64, "each agent counted once");
    let mut expected = Usage::default();
    for agent in &agents {
        expected += agent.stats.usage;
    }
    assert_eq!(total, expected);
}

#[test]
fn a_connection_without_the_counting_function_cannot_write_an_agent() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    drop(database_at(&path, SCHEMA_VERSION));
    let bare = Connection::open(&path).unwrap();
    let agent = whole_agent(RunId::new(), AgentStatus::Running, usage(1, 1));
    let body = serde_json::to_string(&agent).unwrap();
    let written = bare.execute(
        "INSERT INTO agents(id, parent, role, status, run, created_at, updated_at, body)
         VALUES (?1, NULL, 'worker', 'running', ?2, '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z', ?3)",
        [agent.id.to_string(), agent.run.to_string(), body],
    );
    let error = written.expect_err("the trigger needs the function");
    assert!(
        error.to_string().contains("aether_counted_usage"),
        "the failure names what is missing: {error}"
    );
    let rows: i64 = bare
        .query_row("SELECT COUNT(*) FROM agents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0, "nothing was written uncounted");
}
