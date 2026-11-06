//! V33: the fleet's usage moves beside the agents table, backfilled from
//! every row a 1.2.11 store (V28) holds, and kept by triggers from then on.

use super::tests::{database_at, version};
use super::*;
use crate::Store;
use aether_core::model::Usage;

fn usage(input_tokens: u64, cost_usd: f64) -> Usage {
    Usage {
        input_tokens,
        cost_usd,
        cost_complete: true,
        ..Default::default()
    }
}

/// An agent row as any version wrote it: the columns, and a body holding
/// what V33 reads from it.
fn insert_agent(conn: &Connection, id: &str, status: &str, body: &str) {
    conn.execute(
        "INSERT INTO agents(id, parent, role, status, run, created_at, updated_at, body)
         VALUES (?1, NULL, 'worker', ?2, 'run_1', '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z', ?3)",
        [id, status, body],
    )
    .expect("agent row");
}

fn body(id: &str, usage: &Usage) -> String {
    serde_json::json!({ "id": id, "stats": { "usage": usage } }).to_string()
}

#[test]
fn v33_backfills_the_fleet_usage_of_a_1_2_11_store_from_every_readable_row() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let previous = database_at(&path, 28);
    let (running, done) = (usage(10, 1.5), usage(5, 0.25));
    insert_agent(&previous, "agt_a", "running", &body("agt_a", &running));
    insert_agent(&previous, "agt_b", "completed", &body("agt_b", &done));
    insert_agent(
        &previous,
        "agt_c",
        "imported",
        &body("agt_c", &usage(1_000, 9.0)),
    );
    insert_agent(&previous, "agt_d", "failed", "{broken");
    insert_agent(&previous, "agt_e", "created", r#"{"id":"agt_e"}"#);
    insert_agent(
        &previous,
        "agt_f",
        "running",
        &body("agt_other", &usage(7, 7.0)),
    );
    drop(previous);

    let store = crate::SqliteStore::open(&path).expect("a 1.2.11 store opens and migrates");
    assert_eq!(
        store.meta("schema_version").unwrap(),
        Some(SCHEMA_VERSION.to_string())
    );
    let mut expected = Usage::default();
    for counted in [running, done, Usage::default()] {
        expected += counted;
    }
    assert_eq!(store.fleet_usage().unwrap(), expected);
    drop(store);

    let backups: Vec<String> = std::fs::read_dir(directory.path().join("migration-backups"))
        .expect("a backup was taken")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        backups
            .iter()
            .any(|name| name.starts_with("schema-28-before-33-")),
        "{backups:?}"
    );
    let conn = Connection::open(&path).unwrap();
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
fn v33_replays_over_a_rewound_stamp_without_counting_an_agent_twice() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    drop(database_at(&path, SCHEMA_VERSION));
    let conn = Connection::open(&path).unwrap();
    insert_agent(&conn, "agt_a", "running", &body("agt_a", &usage(3, 0.5)));
    conn.execute(
        "UPDATE meta SET value = '32' WHERE key = 'schema_version'",
        [],
    )
    .unwrap();
    migrate(&conn).expect("the replay");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM agent_usage", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(version(&conn), SCHEMA_VERSION);
}

#[test]
fn agent_usage_follows_inserts_updates_imports_and_deletes() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let conn = database_at(&path, SCHEMA_VERSION);
    let read = |conn: &Connection| -> Vec<(String, Option<String>)> {
        conn.prepare("SELECT agent_id, usage FROM agent_usage ORDER BY agent_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    insert_agent(&conn, "agt_a", "running", &body("agt_a", &usage(1, 0.1)));
    insert_agent(&conn, "agt_b", "running", &body("agt_b", &usage(2, 0.2)));
    conn.execute(
        "UPDATE agents SET body = ?1 WHERE id = 'agt_a'",
        [body("agt_a", &usage(4, 0.4))],
    )
    .unwrap();
    let rows = read(&conn);
    assert_eq!(rows.len(), 2);
    let a: Usage = serde_json::from_str(rows[0].1.as_deref().unwrap()).unwrap();
    assert_eq!(a, usage(4, 0.4), "an update replaces the usage");
    conn.execute(
        "UPDATE agents SET status = 'imported' WHERE id = 'agt_a'",
        [],
    )
    .unwrap();
    assert_eq!(read(&conn).len(), 1, "an imported agent is not the fleet's");
    conn.execute("DELETE FROM agents WHERE id = 'agt_b'", [])
        .unwrap();
    assert!(read(&conn).is_empty(), "a deleted agent leaves the total");
}
