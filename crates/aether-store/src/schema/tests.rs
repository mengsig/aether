use super::*;

fn previous_database(path: &std::path::Path) -> Connection {
    database_at(path, SCHEMA_VERSION - 1)
}

fn database_at(path: &std::path::Path, version: u32) -> Connection {
    assert!(version > 0);
    assert!(version <= SCHEMA_VERSION);
    let conn = Connection::open(path).expect("database");
    conn.execute_batch("PRAGMA journal_mode=WAL;").expect("WAL");
    for statement in &MIGRATIONS[..version as usize] {
        conn.execute_batch(statement).expect("previous schema");
    }
    conn.execute(
        "INSERT INTO meta(key,value) VALUES ('schema_version',?1)",
        [version.to_string()],
    )
    .expect("version");
    conn
}

#[test]
fn migration_backup_preserves_wal_data_and_row_ids_before_upgrade() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let previous = previous_database(&path);
    previous.execute_batch("CREATE TABLE backup_probe(key TEXT PRIMARY KEY, body TEXT); INSERT INTO backup_probe VALUES ('first','removed'),('second','committed in WAL'); DELETE FROM backup_probe WHERE key='first';").expect("probe");
    let before = dump(&previous);
    let upgraded = crate::SqliteStore::open(&path).expect("upgrade");
    let backups: Vec<_> = std::fs::read_dir(directory.path().join("migration-backups"))
        .expect("backups")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(backups.len(), 1);
    let (_restore_dir, restored) = restore(&backups[0]);
    let compressed = std::fs::metadata(&backups[0]).expect("backup").len();
    let plain = std::fs::metadata(&restored).expect("restored").len();
    assert!(compressed < plain, "{compressed} >= {plain}");
    let backup = Connection::open_with_flags(&restored, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("backup");
    assert_eq!(dump(&backup), before, "the restored store is identical");
    assert_eq!(version(&backup), SCHEMA_VERSION - 1);
    let probe: (i64, String) = backup
        .query_row(
            "SELECT rowid,body FROM backup_probe WHERE key='second'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("copied WAL row");
    assert_eq!(probe, (2, "committed in WAL".into()));
    let integrity: String = backup
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .expect("integrity");
    assert_eq!(integrity, "ok");
    assert_eq!(
        crate::Store::meta(&upgraded, "schema_version").expect("version"),
        Some(SCHEMA_VERSION.to_string())
    );
    drop(upgraded);
    drop(previous);
    crate::SqliteStore::open(&path).expect("reopen current schema");
    assert_eq!(
        std::fs::read_dir(directory.path().join("migration-backups"))
            .expect("backups")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "zst"))
            .count(),
        1
    );
}

#[test]
fn failed_migration_backup_leaves_the_original_schema_unchanged() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let previous = previous_database(&path);
    std::fs::write(
        directory.path().join("migration-backups"),
        "not a directory",
    )
    .expect("obstruction");
    let error = crate::SqliteStore::open(&path).expect_err("backup must fail");
    assert!(matches!(error, crate::StoreError::MigrationBackup { .. }));
    assert!(error.to_string().contains(path.to_str().expect("path")));
    assert_eq!(version(&previous), SCHEMA_VERSION - 1);
}

#[test]
fn v9_initializes_cancellation_intent_without_overwriting_existing_state() {
    for existing in [None, Some(r#"{"pending":"preserved"}"#)] {
        let conn = Connection::open_in_memory().expect("connection");
        for statements in &MIGRATIONS[..8] {
            conn.execute_batch(statements).expect("v8 schema");
        }
        conn.execute(
            "INSERT INTO meta(key,value) VALUES ('schema_version','8')",
            [],
        )
        .expect("version");
        if let Some(body) = existing {
            conn.execute(
                "INSERT INTO meta(key,value) VALUES ('runtime.pending_cancellations',?1)",
                [body],
            )
            .expect("pending state");
        }
        migrate(&conn).expect("migrate");
        assert_eq!(version(&conn), SCHEMA_VERSION);
        let body: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'runtime.pending_cancellations'",
                [],
                |row| row.get(0),
            )
            .expect("journal");
        assert_eq!(body, existing.unwrap_or("{}"));
    }
}

/// Restore a migration backup as docs/migration-backups.md describes: it is
/// a zstd frame holding the plain SQLite file.
fn restore(backup: &std::path::Path) -> (tempfile::TempDir, std::path::PathBuf) {
    assert!(
        backup.to_string_lossy().ends_with(".sqlite.zst"),
        "{}",
        backup.display()
    );
    let directory = tempfile::tempdir().expect("restore directory");
    let restored = directory.path().join("aether.db");
    zstd::stream::copy_decode(
        std::fs::File::open(backup).expect("backup"),
        std::fs::File::create(&restored).expect("restored"),
    )
    .expect("decompress");
    assert!(std::fs::metadata(&restored).expect("restored").len() > 0);
    (directory, restored)
}

/// Every schema object and every row of every ordinary table, by row ID.
fn dump(conn: &Connection) -> Vec<String> {
    let mut statement = conn
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .expect("schema");
    let objects: Vec<(String, String, Option<String>)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("objects")
        .collect::<rusqlite::Result<_>>()
        .expect("object");
    assert!(!objects.is_empty());
    let mut lines = Vec::new();
    for (kind, name, sql) in objects {
        lines.push(format!("{kind} {name} {sql:?}"));
        let virtual_table = sql
            .as_deref()
            .is_some_and(|sql| sql.starts_with("CREATE VIRTUAL"));
        if kind != "table" || virtual_table || name.starts_with("sqlite_") {
            continue;
        }
        // Row IDs are compared where a table has them; sorting makes order irrelevant.
        let rowid = !sql
            .as_deref()
            .is_some_and(|sql| sql.contains("WITHOUT ROWID"));
        let query = if rowid {
            format!("SELECT rowid, * FROM \"{name}\"")
        } else {
            format!("SELECT * FROM \"{name}\"")
        };
        let mut rows = conn.prepare(&query).expect("rows");
        let columns = rows.column_count();
        let mut cursor = rows.query([]).expect("query");
        let mut table = Vec::new();
        while let Some(row) = cursor.next().expect("row") {
            let values: Vec<rusqlite::types::Value> =
                (0..columns).map(|i| row.get(i).expect("value")).collect();
            table.push(format!("{name} {values:?}"));
        }
        table.sort();
        lines.extend(table);
    }
    lines
}

fn version(conn: &Connection) -> u32 {
    conn.query_row(
        "SELECT value FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(0)
}

/// A store exactly as version 1 wrote it, with one row in it.
fn v1_store_with_data(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )
    .expect("meta");
    conn.execute_batch(V1).expect("v1");
    conn.execute(
        "INSERT INTO events(seq, id, at, type, family, agent_id, body)
         VALUES (1, 'ev_1', '2026-01-01T00:00:00Z', 'agent.started', 'agent', 'agt_1', '{\"kept\":true}')",
        [],
    )
    .expect("event");
    conn.execute(
        "INSERT INTO agents(id, role, status, run, created_at, updated_at, body)
         VALUES ('agt_1', 'root', 'running', 'run_1', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '{\"name\":\"root\"}')",
        [],
    )
    .expect("agent");
    conn.execute(
        "INSERT INTO artifacts(id, agent_id, created_at, body)
         VALUES ('art_1', 'agt_1', '2026-01-01T00:00:00Z', '{\"name\":\"old\"}')",
        [],
    )
    .expect("artifact");
    // A memory as v1 wrote it: no `pinned`, no `forgotten_at`, and a
    // repository scope named by the raw `origin` URL, token and all.
    conn.execute(
        "INSERT INTO memories(id, scope, kind, text, tags, importance, created_at, body)
         VALUES ('mem_01JB000000000000000000000A', 'repo:https://ci-token:s3cret@git.example.com/org/repo.git',
                 'fact', 'The build runs through scripts/ci.sh', '', 0.6, '2026-01-01T00:00:00Z', ?1)",
        [V1_MEMORY_BODY],
    )
    .expect("memory");
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('schema_version', '1')",
        [],
    )
    .expect("stamp");
}

/// The same row's JSON body, written before `pinned` and `forgotten_at`
/// existed and before repository ids were normalized.
const V1_MEMORY_BODY: &str = r#"{"id":"mem_01JB000000000000000000000A",
    "scope":{"scope":"repository","id":"https://ci-token:s3cret@git.example.com/org/repo.git"},
    "kind":"fact","text":"The build runs through scripts/ci.sh","importance":0.6,
    "confidence":0.9,"tags":[],"provenance":{},"created_at":"2026-01-01T00:00:00Z",
    "retrieval_count":0}"#;

fn column_names(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .expect("pragma");
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .expect("query");
    rows.map(|r| r.expect("name")).collect()
}

/// The id reaches the system prompt and the event log, so an upgrade is
/// the last chance to get a credential out of the rows that can still be
/// rewritten. A name that carries no secret is left exactly as it is.
#[test]
fn v8_strips_a_credential_from_an_agents_repository_scope() {
    let conn = Connection::open_in_memory().expect("open");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )
    .expect("meta");
    conn.execute_batch(V1).expect("v1");
    for (id, repository) in [
        (
            "agt_1",
            "https://ci-token:s3cret@git.example.com/org/repo.git",
        ),
        ("agt_2", "already/normal"),
    ] {
        conn.execute(
            "INSERT INTO agents(id, role, status, run, created_at, updated_at, body)
             VALUES (?1, 'root', 'running', 'run_1', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', ?2)",
            rusqlite::params![
                id,
                format!(r#"{{"scope":{{"repository":"{repository}"}}}}"#)
            ],
        )
        .expect("agent");
    }
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('schema_version', '1')",
        [],
    )
    .expect("stamp");

    migrate(&conn).expect("migrate");

    let repository = |id: &str| -> String {
        let body: String = conn
            .query_row("SELECT body FROM agents WHERE id = ?1", [id], |r| r.get(0))
            .expect("agent");
        let value: serde_json::Value = serde_json::from_str(&body).expect("json");
        value["scope"]["repository"]
            .as_str()
            .expect("repository")
            .to_owned()
    };
    assert_eq!(repository("agt_1"), "git.example.com/org/repo");
    assert_eq!(repository("agt_2"), "already/normal");
}

#[test]
fn a_v1_store_with_data_reaches_the_current_version_with_its_data_intact() {
    let conn = Connection::open_in_memory().expect("open");
    v1_store_with_data(&conn);

    migrate(&conn).expect("migrate");
    assert_eq!(version(&conn), SCHEMA_VERSION);

    // The rows that were there are still there, unchanged.
    let body: String = conn
        .query_row("SELECT body FROM events WHERE id = 'ev_1'", [], |r| {
            r.get(0)
        })
        .expect("the event survives the upgrade");
    assert_eq!(body, r#"{"kept":true}"#);
    assert_eq!(
        conn.query_row("SELECT role FROM agents WHERE id = 'agt_1'", [], |r| r
            .get::<_, String>(
            0
        ))
        .expect("the agent survives"),
        "root"
    );
    assert_eq!(
        conn.query_row("SELECT body FROM artifacts WHERE id = 'art_1'", [], |r| {
            r.get::<_, String>(0)
        })
        .expect("the artifact survives"),
        r#"{"name":"old"}"#
    );

    // The whole current shape, not just "the tables exist". A wrong V4 -
    // one that adds `digest` and forgets `evicted` - fails here rather
    // than on the first artifact write against an upgraded store.
    let artifacts = column_names(&conn, "artifacts");
    for column in [
        "id",
        "agent_id",
        "task_id",
        "created_at",
        "body",
        "digest",
        "evicted",
    ] {
        assert!(
            artifacts.contains(&column.to_owned()),
            "artifacts.{column} is missing after migrate: {artifacts:?}"
        );
    }
    assert_eq!(
        conn.query_row(
            "SELECT evicted FROM artifacts WHERE id = 'art_1'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .expect("the new column has a value for old rows"),
        0
    );
    // v7 added two columns to a table that already had rows: an old
    // memory has to default correctly and its pre-v7 body has to still
    // deserialize, which is what makes it retrievable at all.
    let memories = column_names(&conn, "memories");
    for column in ["pinned", "forgotten_at"] {
        assert!(
            memories.contains(&column.to_owned()),
            "memories.{column} is missing after migrate: {memories:?}"
        );
    }
    let (pinned, forgotten, body): (i64, Option<String>, String) = conn
        .query_row(
            "SELECT pinned, forgotten_at, body FROM memories WHERE id = 'mem_01JB000000000000000000000A'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("the memory survives the upgrade");
    assert_eq!(pinned, 0, "an old memory is not pinned");
    assert_eq!(forgotten, None, "an old memory is not forgotten");
    let record: aether_core::memory::MemoryRecord =
        serde_json::from_str(&body).expect("a pre-v7 body still reads back as a MemoryRecord");
    assert!(!record.pinned);
    assert!(record.forgotten_at.is_none());

    // v8: the credential the raw `origin` URL carried is gone from both
    // the indexed key and the body it was written into.
    let scope: String = conn
        .query_row(
            "SELECT scope FROM memories WHERE id = 'mem_01JB000000000000000000000A'",
            [],
            |r| r.get(0),
        )
        .expect("scope");
    assert_eq!(scope, "repo:git.example.com/org/repo");
    assert!(
        !body.contains("s3cret"),
        "the token survived the upgrade: {body}"
    );
    assert_eq!(
        record.scope,
        aether_core::memory::MemoryScope::Repository {
            id: "git.example.com/org/repo".into()
        }
    );

    let outcomes = column_names(&conn, "tool_outcomes");
    for column in [
        "agent_id",
        "invocation_id",
        "key",
        "call_id",
        "tool",
        "at",
        "body",
    ] {
        assert!(
            outcomes.contains(&column.to_owned()),
            "tool_outcomes.{column} is missing: {outcomes:?}"
        );
    }
    // A replay record is keyed by invocation as well as agent, so the
    // same key under two invocations is two rows.
    for invocation in ["inv_1", "inv_2"] {
        conn.execute(
            "INSERT INTO tool_outcomes(agent_id, invocation_id, key, call_id, tool, at, body)
             VALUES ('agt_1', ?1, 'bash:h:aa', 'call_1', 'bash', '2026-01-01T00:00:00Z', '{}')",
            [invocation],
        )
        .expect("record");
    }
    assert_eq!(
        conn.query_row("SELECT count(*) FROM tool_outcomes", [], |r| r
            .get::<_, i64>(0))
            .expect("count"),
        2
    );
    for table in ["approval_rules", "tool_outcomes"] {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_or_else(|e| panic!("{table} missing after migrate: {e}"));
    }
    // Migrating again is a no-op, not an error.
    migrate(&conn).expect("re-migrate");
    assert_eq!(version(&conn), SCHEMA_VERSION);
}

#[test]
fn a_store_already_partway_up_finishes_the_climb() {
    let conn = Connection::open_in_memory().expect("open");
    v1_store_with_data(&conn);
    conn.execute_batch(V2).expect("v2");
    conn.execute_batch(V3).expect("v3");
    conn.execute(
        "UPDATE meta SET value = '3' WHERE key = 'schema_version'",
        [],
    )
    .expect("stamp");
    migrate(&conn).expect("migrate");
    assert_eq!(version(&conn), SCHEMA_VERSION);
    assert!(column_names(&conn, "artifacts").contains(&"evicted".to_owned()));
}

#[test]
fn an_upgrade_that_fails_partway_leaves_the_version_where_it_was() {
    let conn = Connection::open_in_memory().expect("open");
    v1_store_with_data(&conn);
    // V4's `ALTER TABLE artifacts ADD COLUMN digest` cannot run twice.
    // A store where that column already exists makes the block fail the
    // way an interrupted upgrade would.
    conn.execute("ALTER TABLE artifacts ADD COLUMN digest TEXT", [])
        .expect("pre-existing column");

    let failed = migrate(&conn).expect_err("the upgrade must fail, not half-apply");
    assert!(
        failed.to_string().contains("digest"),
        "the error names the statement that failed: {failed}"
    );
    assert_eq!(
        version(&conn),
        1,
        "an interrupted upgrade leaves the store at the version it was"
    );
    assert!(
        conn.query_row("SELECT count(*) FROM approval_rules", [], |r| r
            .get::<_, i64>(0))
            .is_err(),
        "and the blocks before the failure rolled back with it"
    );
    // The data is untouched, and a retry after the obstruction is gone
    // succeeds.
    assert_eq!(
        conn.query_row("SELECT body FROM events WHERE id = 'ev_1'", [], |r| r
            .get::<_, String>(0))
            .expect("the event is still there"),
        r#"{"kept":true}"#
    );
    conn.execute("ALTER TABLE artifacts DROP COLUMN digest", [])
        .expect("clear the obstruction");
    migrate(&conn).expect("a retry succeeds");
    assert_eq!(version(&conn), SCHEMA_VERSION);
}

#[test]
fn a_meta_read_that_fails_is_not_read_as_a_fresh_store() {
    let conn = Connection::open_in_memory().expect("open");
    v1_store_with_data(&conn);
    conn.execute(
        "UPDATE meta SET value = 'not-a-number' WHERE key = 'schema_version'",
        [],
    )
    .expect("corrupt the stamp");
    let err = migrate(&conn).expect_err("a corrupt stamp must not read as version 0");
    assert!(
        err.to_string().contains("not-a-number"),
        "the error names the real problem: {err}"
    );
}

#[test]
fn a_store_from_the_future_is_refused() {
    let conn = Connection::open_in_memory().expect("open");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )
    .expect("meta");
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)",
        [(SCHEMA_VERSION + 1).to_string()],
    )
    .expect("stamp");
    assert!(matches!(
        migrate(&conn),
        Err(StoreError::SchemaTooNew { .. })
    ));
}

#[test]
fn v12_keeps_schedules_and_history_and_accepts_targetless_rows() {
    let conn = Connection::open_in_memory().expect("connection");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("foreign keys");
    for statements in &MIGRATIONS[..11] {
        conn.execute_batch(statements).expect("v11 schema");
    }
    conn.execute_batch(
        "INSERT INTO meta(key,value) VALUES ('schema_version','11');
         INSERT INTO schedules VALUES ('sch_a','agt_a','2026-09-09T12:00:00.000000000Z','{}');
         INSERT INTO schedule_occurrences VALUES ('sch_a','2026-09-09T11:00:00.000000000Z','{}');",
    )
    .expect("v11 rows");
    migrate(&conn).expect("migrate");
    assert_eq!(version(&conn), SCHEMA_VERSION);
    let kept: (String, String, i64) = conn
        .query_row(
            "SELECT s.target, s.next_due, (SELECT count(*) FROM schedule_occurrences o WHERE o.schedule = s.id)
             FROM schedules s WHERE s.id = 'sch_a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("migrated schedule");
    assert_eq!(
        kept,
        ("agt_a".into(), "2026-09-09T12:00:00.000000000Z".into(), 1)
    );
    conn.execute(
        "INSERT INTO schedules(id, target, next_due, body) VALUES ('sch_b', NULL, NULL, '{}')",
        [],
    )
    .expect("targetless schedule");
    // The rebuilt history table still refuses rows for unknown schedules.
    assert!(
        conn.execute(
            "INSERT INTO schedule_occurrences VALUES ('sch_missing','2026-09-09T11:00:00Z','{}')",
            [],
        )
        .is_err()
    );
    let indexes: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('schedules_due','schedules_target')",
            [],
            |row| row.get(0),
        )
        .expect("indexes");
    assert_eq!(indexes, 2);
}

#[test]
fn artifact_cleanup_migration_tracks_pending_unlinks_and_indexes_live_paths() {
    let dir = tempfile::tempdir().unwrap();
    let conn = previous_database(&dir.path().join("aether.db"));
    conn.execute(
        "INSERT INTO artifacts(id,created_at,evicted,body) VALUES ('old','now',1,'{}')",
        [],
    )
    .unwrap();
    migrate(&conn).unwrap();
    let pending: bool = conn
        .query_row(
            "SELECT unlink_pending FROM artifacts WHERE id = 'old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(pending, "pre-upgrade interrupted evictions must be retried");
    conn.execute(
        "UPDATE artifacts SET unlink_pending = 0 WHERE id = 'old'",
        [],
    )
    .unwrap();
    migrate(&conn).unwrap();
    let pending: bool = conn
        .query_row(
            "SELECT unlink_pending FROM artifacts WHERE id = 'old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!pending, "already confirmed unlinks must stay confirmed");
    conn.execute(
        "INSERT INTO artifacts(id,created_at,body) VALUES ('live','now','{\"path\":\"payload\"}')",
        [],
    )
    .unwrap();
    conn.execute("UPDATE artifacts SET evicted = 1 WHERE id = 'live'", [])
        .unwrap();
    let pending: bool = conn
        .query_row(
            "SELECT unlink_pending FROM artifacts WHERE id = 'live'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(pending);
    for (query, index) in [
        (
            "EXPLAIN QUERY PLAN SELECT 1 FROM artifacts WHERE evicted = 0 AND json_extract(body, '$.path') = 'payload'",
            "artifacts_live_path",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT body FROM artifacts WHERE unlink_pending = 1 AND id > '' ORDER BY id LIMIT 1000",
            "artifacts_unlink_pending",
        ),
    ] {
        let plans = conn
            .prepare(query)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(plans.iter().any(|plan| plan.contains(index)), "{plans:?}");
    }
}

/// The last schema of a binary whose content blocks have no `image_ref`.
const LAST_SCHEMA_WITHOUT_IMAGE_REFS: u32 = 18;

#[test]
fn v19_backs_up_a_v18_store_and_keeps_binaries_without_image_refs_out() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let v18 = Connection::open(&path).expect("database");
    for statements in &MIGRATIONS[..LAST_SCHEMA_WITHOUT_IMAGE_REFS as usize] {
        v18.execute_batch(statements).expect("v18 schema");
    }
    v18.execute_batch(
        "INSERT INTO meta(key,value) VALUES ('schema_version','18');
         INSERT INTO inbox(id,agent_id,seq,kind,delivered_at,body)
             VALUES ('inb_a','agt_a',1,'message','2026-10-12T00:00:00Z','{\"text\":\"kept\"}');",
    )
    .expect("v18 rows");
    drop(v18);

    let upgraded = crate::SqliteStore::open(&path).expect("upgrade");
    let stamp = crate::Store::meta(&upgraded, "schema_version").expect("version");
    assert_eq!(stamp, Some(SCHEMA_VERSION.to_string()));
    drop(upgraded);

    let backups: Vec<_> = std::fs::read_dir(directory.path().join("migration-backups"))
        .expect("backups")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(backups.len(), 1, "one snapshot of the version 18 store");
    let name = backups[0].file_name().expect("name").to_string_lossy();
    let prefix = format!("schema-18-before-{SCHEMA_VERSION}-");
    assert!(name.starts_with(&prefix), "{name}");
    let (_restore_dir, restored) = restore(&backups[0]);
    let backup = Connection::open_with_flags(&restored, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("backup");
    assert_eq!(version(&backup), LAST_SCHEMA_WITHOUT_IMAGE_REFS);

    let conn = Connection::open(&path).expect("reopen");
    let body: String = conn
        .query_row("SELECT body FROM inbox WHERE id = 'inb_a'", [], |row| {
            row.get(0)
        })
        .expect("the message survives the upgrade");
    assert_eq!(body, r#"{"text":"kept"}"#);
    // The check `migrate` makes, with the older binary's own supported version.
    let refusal = refuse_newer_store(Some(version(&conn)), LAST_SCHEMA_WITHOUT_IMAGE_REFS);
    assert!(matches!(
        refusal,
        Err(StoreError::SchemaTooNew {
            found: SCHEMA_VERSION,
            supported: 18
        })
    ));
}

/// A memory filed under an old name before aliases existed is kept, and can
/// be moved once the store holds the alias table.
#[test]
fn v20_adds_repository_aliases_to_a_populated_v19_store() {
    use aether_core::memory::{MemoryCandidate, MemoryKind, MemoryRecord, MemoryScope};
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let v19 = database_at(&path, 19);
    assert_eq!(version(&v19), 19);
    let scope = MemoryScope::Repository {
        id: "archon".into(),
    };
    let record = MemoryRecord::from_candidate(
        MemoryCandidate {
            scope: scope.clone(),
            kind: MemoryKind::Fact,
            text: "filed before aliases".into(),
            structured: None,
            importance: 0.5,
            confidence: 0.5,
            tags: Vec::new(),
            provenance: Default::default(),
            supersedes: None,
        },
        aether_core::time::now(),
    );
    v19.execute(
        "INSERT INTO memories(id, scope, kind, text, tags, importance, created_at, body)
         VALUES (?1, ?2, 'fact', ?3, '', 0.5, ?4, ?5)",
        rusqlite::params![
            record.id.to_string(),
            "repo:archon",
            record.text,
            record.created_at.to_rfc3339(),
            serde_json::to_string(&record).expect("body"),
        ],
    )
    .expect("v19 memory");
    drop(v19);

    let store = crate::SqliteStore::open(&path).expect("upgrade");
    let stamp = crate::Store::meta(&store, "schema_version").expect("version");
    assert_eq!(stamp, Some(SCHEMA_VERSION.to_string()));
    assert!(
        crate::Store::repository_aliases(&store)
            .expect("aliases")
            .is_empty()
    );
    assert_eq!(
        crate::Store::memory(&store, record.id).expect("read"),
        Some(record.clone())
    );
    let backups: Vec<_> = std::fs::read_dir(directory.path().join("migration-backups"))
        .expect("backups")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        backups
            .iter()
            .any(|name| name.starts_with(&format!("schema-19-before-{SCHEMA_VERSION}-"))),
        "{backups:?}"
    );

    let alias = aether_core::repository::RepositoryAlias::new(
        "archon",
        "github.com/mengsig/archon",
        aether_core::time::now(),
    )
    .expect("alias");
    let gate = crate::WriteGate::new();
    let moved = crate::Store::alias_repository(
        &store,
        &alias,
        aether_core::event::Actor::System,
        &gate.hold(),
    )
    .expect("alias the migrated store");
    assert_eq!(moved.counts.repository, 1);
    let after = crate::Store::memory(&store, record.id)
        .expect("read")
        .expect("kept");
    assert_eq!(
        after.scope,
        MemoryScope::Repository {
            id: alias.canonical
        }
    );
}

fn insert_legacy_memory(conn: &Connection, record: &aether_core::memory::MemoryRecord) {
    assert!(record.proposed_at.is_none());
    assert!(record.accepted_at.is_none());
    conn.execute(
        "INSERT INTO memories(id, scope, kind, text, tags, importance, created_at, body, pinned, forgotten_at, superseded_by)
         VALUES (?1, 'global', 'fact', ?2, '', ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![record.id.to_string(), record.text, record.importance,
            record.created_at.to_rfc3339(), serde_json::to_string(record).unwrap(),
            record.pinned, record.forgotten_at.map(|at| at.to_rfc3339()),
            record.superseded_by.map(|id| id.to_string())],
    ).expect("legacy memory");
}

#[test]
fn v21_migrates_populated_v20_and_excludes_new_proposals_from_default_queries() {
    use crate::{Commit, MemoryQuery, Mutation, Store};
    use aether_core::{
        id::MemoryId,
        memory::{MemoryRecord, MemoryScope},
        time::now,
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("aether.db");
    let v20 = database_at(&path, 20);
    assert_eq!(version(&v20), 20);
    let mut legacy: MemoryRecord = serde_json::from_str(V1_MEMORY_BODY).unwrap();
    legacy.scope = MemoryScope::Global;
    let mut records = Vec::new();
    for state in ["active", "pinned", "forgotten", "superseded"] {
        let mut record = legacy.clone();
        record.id = MemoryId::new();
        record.pinned = state == "pinned";
        record.forgotten_at = (state == "forgotten").then(now);
        record.superseded_by = (state == "superseded").then(MemoryId::new);
        insert_legacy_memory(&v20, &record);
        records.push(record);
    }
    drop(v20);
    let store = crate::SqliteStore::open(&path).unwrap();
    assert_eq!(
        store.meta("schema_version").unwrap(),
        Some(SCHEMA_VERSION.to_string())
    );
    for record in &records {
        assert_eq!(store.memory(record.id).unwrap().as_ref(), Some(record));
    }
    assert_eq!(store.memories(&MemoryQuery::default()).unwrap().len(), 2);
    let mut proposal = legacy;
    proposal.id = MemoryId::new();
    proposal.proposed_at = Some(now());
    store
        .commit(Commit::new().mutate(Mutation::PutMemory(proposal.clone())))
        .unwrap();
    assert_eq!(store.memories(&MemoryQuery::default()).unwrap().len(), 2);
    assert_eq!(
        store
            .memories(&MemoryQuery {
                proposed: true,
                ..Default::default()
            })
            .unwrap(),
        vec![proposal]
    );
    let conn = Connection::open(&path).unwrap();
    assert_v20_refuses_proposals(&conn);
    let projection = conn
        .query_row("SELECT count(*), sum(proposed) FROM memories", [], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .unwrap();
    assert_eq!(projection, (5, 1));
}

fn assert_v20_refuses_proposals(conn: &Connection) {
    assert_eq!(version(conn), SCHEMA_VERSION);
    assert!(matches!(
        refuse_newer_store(current_version(conn).unwrap(), 20),
        Err(StoreError::SchemaTooNew {
            found: SCHEMA_VERSION,
            supported: 20
        })
    ));
}

#[test]
fn v22_feedback_migration_preserves_v21_memories_and_saves_a_readable_backup() {
    use crate::Store;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("aether.db");
    let v21 = database_at(&path, 21);
    let record: aether_core::memory::MemoryRecord = serde_json::from_str(V1_MEMORY_BODY).unwrap();
    insert_legacy_memory(&v21, &record);
    assert_eq!(version(&v21), 21);
    drop(v21);
    let store = crate::SqliteStore::open(&path).unwrap();
    assert_eq!(
        store.meta("schema_version").unwrap(),
        Some(SCHEMA_VERSION.to_string())
    );
    assert_eq!(store.memory(record.id).unwrap(), Some(record.clone()));
    let connection = Connection::open(&path).unwrap();
    assert_feedback_columns(&connection);
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM memory_feedback", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(store);
    let conn = Connection::open(&path).unwrap();
    let index: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'memory_feedback_memory_created'",
        [], |row| row.get(0),
    ).unwrap();
    assert!(index.contains("memory_id, created_at"));
    assert!(matches!(
        refuse_newer_store(current_version(&conn).unwrap(), 21),
        Err(StoreError::SchemaTooNew {
            found: SCHEMA_VERSION,
            supported: 21
        })
    ));
    let backups: Vec<_> = std::fs::read_dir(directory.path().join("migration-backups"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1);
    assert!(
        backups[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(&format!("schema-21-before-{SCHEMA_VERSION}-"))
    );
    assert_feedback_backup(&backups[0], &record);
}

fn assert_feedback_backup(path: &std::path::Path, record: &aether_core::memory::MemoryRecord) {
    let (_restore_dir, restored) = restore(path);
    let backup = Connection::open(restored).unwrap();
    assert_eq!(version(&backup), 21);
    assert!(backup.prepare("SELECT * FROM memory_feedback").is_err());
    assert_eq!(
        backup
            .query_row(
                "SELECT body FROM memories WHERE id = ?1",
                [record.id.to_string()],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        serde_json::to_string(&record).unwrap()
    );
}

fn assert_feedback_columns(conn: &Connection) {
    let mut statement = conn.prepare("PRAGMA table_info(memory_feedback)").unwrap();
    let columns: Vec<(String, String, bool)> = statement
        .query_map([], |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for name in ["task_id", "verdict", "evidence"] {
        assert!(columns.contains(&(name.into(), "TEXT".into(), true)));
    }
    assert_eq!(columns.len(), 8);
}

#[test]
fn fresh_v22_feedback_requires_valid_ranking_columns() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();
    assert_eq!(version(&conn), SCHEMA_VERSION);
    assert_feedback_columns(&conn);
    let sql = "INSERT INTO memory_feedback
        (id, memory_id, task_id, verdict, evidence, created_at, body)
        VALUES ('feedback', 'memory', ?1, ?2, ?3, 'timestamp', '{}')";
    for (task, verdict, evidence) in [
        (None, Some("helped"), Some("operator")),
        (Some("task"), None, Some("operator")),
        (Some("task"), Some("helped"), None),
        (Some("task"), Some("unknown"), Some("operator")),
        (Some("task"), Some("helped"), Some("unknown")),
    ] {
        assert!(matches!(
            conn.execute(sql, rusqlite::params![task, verdict, evidence]),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation
        ));
    }
    assert_eq!(
        conn.execute(sql, ["task", "neutral", "outcome"]).unwrap(),
        1
    );
}

#[test]
fn a_v22_store_gains_the_creation_order_listing_indexes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("aether.db");
    let v22 = database_at(&path, 22);
    let indexes = |conn: &Connection| -> Vec<String> {
        conn.prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name IN ('agents', 'tasks') AND sql IS NOT NULL ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let before = indexes(&v22);
    assert!(before.contains(&"agents_run".to_owned()), "{before:?}");
    assert!(!before.contains(&"agents_created".to_owned()), "{before:?}");
    migrate(&v22).unwrap();
    assert_eq!(version(&v22), SCHEMA_VERSION);
    // `agents_run_created` answers every `run = ?` lookup `agents_run` did.
    assert_eq!(
        indexes(&v22),
        [
            "agents_created",
            "agents_parent",
            "agents_run_created",
            "agents_status",
            "tasks_created",
            "tasks_owner",
            "tasks_parent",
            "tasks_run_created",
            "tasks_status",
        ]
    );
}

#[test]
fn v25_gives_lease_and_memory_events_the_run_of_their_agent() {
    let directory = tempfile::tempdir().expect("directory");
    let conn = database_at(&directory.path().join("aether.db"), 24);
    conn.execute_batch(
        "INSERT INTO agents(id,parent,role,status,run,created_at,updated_at,body)
             VALUES ('agt_a',NULL,'root','completed','run_a','2026-10-01T00:00:00Z','2026-10-01T00:00:00Z','{}');
         INSERT INTO events(seq,id,at,type,family,agent_id,run,body) VALUES
             (1,'ev_1','2026-10-01T00:00:00Z','lease.renewed','lease','agt_a',NULL,'{\"agent_id\":\"agt_a\",\"run\":null}'),
             (2,'ev_2','2026-10-01T00:00:00Z','lease.renewed','lease','agt_a','run_other','{\"run\":\"run_other\"}'),
             (3,'ev_3','2026-10-01T00:00:00Z','lease.renewed','lease','agt_gone',NULL,'{}'),
             (4,'ev_4','2026-10-01T00:00:00Z','agent.ready','agent','agt_a',NULL,'{}'),
             (5,'ev_5','2026-10-01T00:00:00Z','memory.rejected','memory','agt_a',NULL,'{}');",
    )
    .expect("seed");

    migrate(&conn).expect("migrate");
    assert_eq!(version(&conn), SCHEMA_VERSION);

    let row = |id: &str| -> (Option<String>, String) {
        conn.query_row("SELECT run, body FROM events WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .expect("event")
    };
    let (run, body) = row("ev_1");
    assert_eq!(run.as_deref(), Some("run_a"));
    let body: serde_json::Value = serde_json::from_str(&body).expect("body");
    assert_eq!(body["run"], "run_a");
    assert_eq!(row("ev_2").0.as_deref(), Some("run_other"));
    assert_eq!(row("ev_3").0, None, "no agent row, no run to give");
    assert_eq!(
        row("ev_4").0,
        None,
        "only the kinds that omitted it are rewritten"
    );
    assert_eq!(row("ev_5").0.as_deref(), Some("run_a"));
}

/// A v26 store whose events carry large tool outputs inline, as every store
/// written before blob offload does.
fn v26_store_with_inline_outputs(path: &std::path::Path) -> Vec<aether_core::event::Event> {
    use aether_core::event::{Actor, EventDraft, EventKind};
    use aether_core::id::{AgentId, ToolCallId};
    use aether_core::tool::ToolOutcome;
    let source = crate::SqliteStore::open_in_memory().expect("source store");
    let agent = AgentId::new();
    let output: String = (0..500).map(|n| format!("legacy output {n}\n")).collect();
    let mut commit = crate::Commit::new();
    for _ in 0..3 {
        commit = commit.event(
            EventDraft::new(
                Actor::System,
                EventKind::ToolSucceeded {
                    call: ToolCallId::new(),
                    outcome: ToolOutcome::ok(output.clone()),
                },
            )
            .agent(agent),
        );
    }
    // Content shaped like a reference to the blob this output becomes once
    // a v27 store writes it: it must still read back as the text it is.
    use sha2::Digest;
    let lookalike = format!(
        "\u{1}aether-blob:{}",
        hex::encode(sha2::Sha256::digest(output.as_bytes()))
    );
    commit = commit.event(
        EventDraft::new(
            Actor::System,
            EventKind::Custom {
                name: "legacy.note".into(),
                payload: serde_json::json!({ "text": lookalike }),
            },
        )
        .agent(agent),
    );
    let events = crate::Store::commit(&source, commit).expect("events");
    let conn = database_at(path, 26);
    for event in &events {
        conn.execute(
            "INSERT INTO events(seq, id, at, type, family, agent_id, run, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
            rusqlite::params![
                event.seq as i64,
                event.id.to_string(),
                event.at.to_rfc3339(),
                event.kind.type_name(),
                event.kind.family(),
                agent.to_string(),
                serde_json::to_string(event).expect("inline body"),
            ],
        )
        .expect("legacy event");
    }
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES ('last_seq', ?1)",
        [events.len().to_string()],
    )
    .expect("last seq");
    events
}

#[test]
fn v27_keeps_inline_outputs_readable_and_offloads_new_ones() {
    use crate::Store;
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let legacy = v26_store_with_inline_outputs(&path);
    let agent = legacy[0].agent_id.expect("agent");

    let store = crate::SqliteStore::open(&path).expect("upgrade");
    let conn = Connection::open(&path).expect("inspect");
    assert_eq!(version(&conn), SCHEMA_VERSION);
    let read = store
        .events(0, 10, &crate::EventFilter::for_agent(agent))
        .expect("legacy history");
    assert_eq!(read, legacy, "inline bodies read back unchanged");

    let mut fresh = legacy[0].clone();
    fresh.id = aether_core::id::EventId::new();
    let committed = store
        .commit(
            crate::Commit::new().event(
                aether_core::event::EventDraft::new(fresh.actor.clone(), fresh.kind.clone())
                    .agent(agent),
            ),
        )
        .expect("new output");
    let blobs: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_blobs", [], |row| row.get(0))
        .expect("blobs");
    assert_eq!(blobs, 1, "a new large output is stored as a blob");
    assert_eq!(
        store
            .event_by_seq(committed[0].seq)
            .expect("read")
            .expect("event")
            .kind,
        legacy[0].kind
    );
    let lookalike = legacy.last().expect("lookalike");
    assert_eq!(
        store.event_by_seq(lookalike.seq).expect("read").as_ref(),
        Some(lookalike),
        "inline content is never resolved as a reference, even to a blob that exists"
    );
    let deleted = conn
        .execute("DELETE FROM events WHERE seq <= 4", [])
        .expect("legacy rows delete without references");
    assert_eq!(deleted, 4);
}

/// The upgrade lane rewinds a v27 store by its stamp alone (REWINDS in
/// scripts/upgrade_compat_migration.py): V27 must replay over its own tables.
#[test]
fn v27_replays_over_a_store_whose_stamp_was_rewound() {
    use crate::Store;
    use aether_core::event::{Actor, EventDraft, EventKind};
    use aether_core::id::{AgentId, ToolCallId};
    use aether_core::tool::ToolOutcome;
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("aether.db");
    let output: String = (0..500)
        .map(|n| format!("offloaded output {n}\n"))
        .collect();
    let committed = {
        let store = crate::SqliteStore::open(&path).expect("v27 store");
        let draft = EventDraft::new(
            Actor::System,
            EventKind::ToolSucceeded {
                call: ToolCallId::new(),
                outcome: ToolOutcome::ok(output),
            },
        )
        .agent(AgentId::new());
        store
            .commit(crate::Commit::new().event(draft))
            .expect("event")
    };
    let conn = Connection::open(&path).expect("rewind");
    conn.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
        [(SCHEMA_VERSION - 1).to_string()],
    )
    .expect("stamp");
    drop(conn);

    let store = crate::SqliteStore::open(&path).expect("replayed upgrade");
    let conn = Connection::open(&path).expect("inspect");
    assert_eq!(version(&conn), SCHEMA_VERSION);
    let blobs: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_blobs", [], |row| row.get(0))
        .expect("blobs");
    assert_eq!(blobs, 1, "the replay keeps the blob the output moved into");
    let read = store.event_by_seq(committed[0].seq).expect("read");
    assert_eq!(
        read.as_ref(),
        Some(&committed[0]),
        "the reference still resolves"
    );
}

#[test]
fn a_v27_store_gains_the_approvals_agent_index_history_retention_reads() {
    let directory = tempfile::tempdir().unwrap();
    let v27 = database_at(&directory.path().join("aether.db"), 27);
    let plan = |conn: &Connection| -> String {
        conn.prepare(
            "EXPLAIN QUERY PLAN SELECT id FROM approvals \
             WHERE agent_id IN (SELECT id FROM agents WHERE run=?1)",
        )
        .unwrap()
        .query_map(["run"], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("; ")
    };
    assert!(!plan(&v27).contains("approvals_agent"), "{}", plan(&v27));
    migrate(&v27).unwrap();
    assert_eq!(version(&v27), SCHEMA_VERSION);
    assert!(
        plan(&v27).contains("USING INDEX approvals_agent"),
        "{}",
        plan(&v27)
    );
}
