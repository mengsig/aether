use super::*;

fn open_temp() -> (tempfile::TempDir, NoesisStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = NoesisStore::open(&dir.path().join("noesis.db")).unwrap();
    (dir, store)
}

fn tables(conn: &Connection) -> Vec<String> {
    let mut statement = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap();
    let names = statement.query_map([], |row| row.get(0)).unwrap();
    names.collect::<Result<_, _>>().unwrap()
}

fn add_session(conn: &Connection, id: &str) {
    conn.execute(
        "INSERT INTO sessions VALUES (?1, 'learn rust', 'teach', 'probe', 'agt_1', 't0', 't0')",
        [id],
    )
    .unwrap();
}

#[test]
fn n1_creates_every_table_the_contract_names() {
    let (_dir, store) = open_temp();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let expected = [
        "chain_premises",
        "chain_provenance",
        "chains",
        "cluster_members",
        "clusters",
        "coverage",
        "jobs",
        "meta",
        "plans",
        "problems",
        "proposition_aliases",
        "proposition_provenance",
        "propositions",
        "questions",
        "session_messages",
        "sessions",
        "sources",
    ];
    assert_eq!(tables(store.connection()), expected);
}

#[test]
fn the_file_is_private_to_its_owner() {
    let (dir, store) = open_temp();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("noesis.db"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(store.path().ends_with("noesis.db"));
    store.close().unwrap();
}

#[test]
fn reopening_keeps_the_rows_and_does_not_migrate_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noesis.db");
    let store = NoesisStore::open(&path).unwrap();
    add_session(store.connection(), "ns_1");
    store.close().unwrap();
    let store = NoesisStore::open(&path).unwrap();
    let count: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn a_store_from_a_newer_binary_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noesis.db");
    NoesisStore::open(&path).unwrap().close().unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE meta SET value = '9' WHERE key = 'schema_version'",
        [],
    )
    .unwrap();
    drop(conn);
    let error = NoesisStore::open(&path).unwrap_err();
    assert!(
        matches!(
            error,
            StoreError::SchemaTooNew {
                found: 9,
                supported: SCHEMA_VERSION
            }
        ),
        "{error}"
    );
    assert!(error.to_string().contains("upgrade Aether"), "{error}");
}

#[test]
fn a_failed_migration_leaves_the_store_unversioned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noesis.db");
    let conn = Connection::open(&path).unwrap();
    // A table N1 creates makes its batch fail half-way through.
    conn.execute_batch("CREATE TABLE jobs (x TEXT);").unwrap();
    drop(conn);
    assert!(matches!(
        NoesisStore::open(&path),
        Err(StoreError::Sqlite(_))
    ));
    let conn = Connection::open(&path).unwrap();
    assert_eq!(schema::current_version(&conn).unwrap(), None);
    assert!(!tables(&conn).contains(&"sessions".to_string()));
}

#[test]
fn a_directory_in_place_of_the_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noesis.db");
    std::fs::create_dir(&path).unwrap();
    let error = NoesisStore::open(&path).unwrap_err();
    assert!(matches!(error, StoreError::Io { .. }), "{error}");
}

#[test]
fn a_session_holds_one_open_question_and_one_active_problem() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    add_session(conn, "ns_1");
    let pose = |id: &str| {
        conn.execute(
            "INSERT INTO questions (id, session, kind, prompt, answer_key, posed_at)
             VALUES (?1, 'ns_1', 'recall', 'why?', '\"because\"', 't1')",
            [id],
        )
    };
    pose("q_1").unwrap();
    assert!(pose("q_2").is_err(), "a second open question was accepted");
    conn.execute(
        "UPDATE questions SET answer = '{}', answered_at = 't2' WHERE id = 'q_1'",
        [],
    )
    .unwrap();
    pose("q_2").unwrap();
    let problem = |id: &str| {
        conn.execute(
            "INSERT INTO problems (id, session, statement, mode, solution, posed_at)
             VALUES (?1, 'ns_1', 'prove it', 'recall', 'the proof', 't1')",
            [id],
        )
    };
    problem("pr_1").unwrap();
    assert!(
        problem("pr_2").is_err(),
        "a second active problem was accepted"
    );
}

#[test]
fn checks_refuse_values_outside_the_wire_enums() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    let bad_session = conn.execute(
        "INSERT INTO sessions VALUES ('ns_1', 'g', 'lecture', 'probe', 'agt_1', 't', 't')",
        [],
    );
    assert!(bad_session.is_err());
    let bad_prefix = conn.execute(
        "INSERT INTO sessions VALUES ('x_1', 'g', 'teach', 'probe', 'agt_1', 't', 't')",
        [],
    );
    assert!(bad_prefix.is_err());
    let bad_job = conn.execute(
        "INSERT INTO jobs (id, kind, target, state, created_at, updated_at)
         VALUES ('j_1', 'extract_session', 'ns_1', 'paused', 't', 't')",
        [],
    );
    assert!(bad_job.is_err());
}

#[test]
fn deleting_a_proposition_takes_its_chains_and_a_session_its_rows() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    add_session(conn, "ns_1");
    conn.execute_batch(
        "INSERT INTO propositions VALUES ('p_a', 'a', 'A holds', 't'), ('p_b', 'b', 'B holds', 't'),
                                         ('p_c', 'c', 'C holds', 't');
         INSERT INTO chains VALUES ('c_1', 'p_c', 't');
         INSERT INTO chain_premises VALUES ('c_1', 'p_a'), ('c_1', 'p_b');
         INSERT INTO proposition_provenance VALUES ('p_a', 'ns_1', 'm1');
         INSERT INTO plans VALUES ('ns_1', 0, '{\"nodes\":[],\"edges\":[]}', 't');",
    )
    .unwrap();
    conn.execute("DELETE FROM propositions WHERE id = 'p_a'", [])
        .unwrap();
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |row| row.get(0)).unwrap() };
    assert_eq!(count("SELECT count(*) FROM chains"), 0);
    assert_eq!(count("SELECT count(*) FROM chain_premises"), 0);
    assert_eq!(count("SELECT count(*) FROM propositions"), 2);
    conn.execute("DELETE FROM sessions WHERE id = 'ns_1'", [])
        .unwrap();
    assert_eq!(count("SELECT count(*) FROM plans"), 0);
}
