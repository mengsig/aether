//! V32, the empty Memory 2.0 slot: it adds nothing, and the backup taken
//! before it is what a rollback to the previous binary restores.

use super::tests::{database_at, dump, restore, v30_store_with_memories, version};
use super::*;
use crate::Store;

/// A V31 store with memories in every Memory 2.0 table, written in SQL so
/// nothing migrates it further; returns the records and the dump.
fn v31_store(path: &std::path::Path) -> (Vec<aether_core::memory::MemoryRecord>, Vec<String>) {
    let (records, _) = v30_store_with_memories(path);
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(V31).unwrap();
    conn.execute_batch(&format!(
        "INSERT INTO memory_stale(memory_id, reason, since, revision)
           VALUES ('{}', 'notes.md no longer states it', '2026-11-01T00:00:00Z', 'abc123');
         INSERT INTO memory_holdouts(memory_id, agent_id, task_id, unit, at, rate, score)
           VALUES ('{}', 'agt_00000000000000000000000000', NULL, 'agt_00000000000000000000000000',
                   '2026-11-01T00:00:00Z', 0.05, 0.7);
         UPDATE meta SET value = '31' WHERE key = 'schema_version';",
        records[1].id, records[2].id
    ))
    .unwrap();
    assert_eq!(version(&conn), 31);
    let dumped = dump(&conn);
    (records, dumped)
}

#[test]
fn the_backup_taken_before_v32_is_a_v31_store_the_previous_binary_opens_with_its_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("aether.db");
    let (records, before) = v31_store(&path);

    let store = crate::SqliteStore::open(&path).unwrap();
    assert_eq!(
        store.meta("schema_version").unwrap(),
        Some(SCHEMA_VERSION.to_string())
    );
    for record in &records {
        assert_eq!(store.memory(record.id).unwrap().as_ref(), Some(record));
    }
    drop(store);

    let backups: Vec<_> = std::fs::read_dir(directory.path().join("migration-backups"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1);
    let name = backups[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with(&format!("schema-31-before-{SCHEMA_VERSION}-")),
        "{name}"
    );
    let (_restored_dir, restored) = restore(&backups[0]);
    let backup =
        Connection::open_with_flags(&restored, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(version(&backup), 31);
    assert_eq!(dump(&backup), before, "every V31 row is in the backup");
    // The previous binary supports 31: it opens the backup and refuses the
    // migrated store, which is why a rollback restores the backup.
    assert!(refuse_newer_store(current_version(&backup).unwrap(), 31).is_ok());
    let migrated = Connection::open(&path).unwrap();
    assert!(matches!(
        refuse_newer_store(current_version(&migrated).unwrap(), 31),
        Err(StoreError::SchemaTooNew {
            found: SCHEMA_VERSION,
            supported: 31
        })
    ));
    // Opening a store also stamps meta and rebuilds the FTS shadow rows, and
    // V33 adds its usage tables, archived indexes and column; V32 adds nothing.
    let migration_visible = |rows: Vec<String>| -> Vec<String> {
        rows.into_iter()
            .filter(|line| !line.starts_with("meta ") && !line.starts_with("memories_fts"))
            .filter(|line| !line.contains("agent_usage") && !line.contains("usage_totals"))
            .filter(|line| !line.contains("archived_id ON agents"))
            .map(|line| line.replace(", archived INTEGER NOT NULL DEFAULT 0)", ")"))
            .collect()
    };
    let after = migration_visible(dump(&migrated));
    assert!(after.iter().any(|line| line.starts_with("memory_stale ")));
    assert_eq!(
        after,
        migration_visible(before),
        "V32 adds and rewrites nothing"
    );
}

/// The upgrade lane undoes V32 with REWINDS[32] (only the stamp); it replays.
#[test]
fn v32_replays_over_the_upgrade_lanes_undo() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("aether.db");
    drop(database_at(&path, SCHEMA_VERSION));
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("UPDATE meta SET value = '31' WHERE key = 'schema_version';")
        .unwrap();
    drop(conn);
    let store = crate::SqliteStore::open(&path).unwrap();
    assert_eq!(
        store.meta("schema_version").unwrap(),
        Some(SCHEMA_VERSION.to_string())
    );
    assert!(store.memory_v2_counts().unwrap().total == 0);
}
