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
        "INSERT INTO sessions(id, goal, mode, phase, agent, created_at, updated_at)
         VALUES (?1, 'learn rust', 'teach', 'probe', 'agt_1', 't0', 't0')",
        [id],
    )
    .unwrap();
}

#[test]
fn the_migrations_create_every_table_the_contract_names() {
    let (_dir, store) = open_temp();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let expected = [
        "chain_premises",
        "chain_provenance",
        "chains",
        "cluster_members",
        "clusters",
        "concept_state",
        "concepts",
        "coverage",
        "evidence",
        "evidence_bundles",
        "evidence_items",
        "jobs",
        "meta",
        "plans",
        "problems",
        "proposition_aliases",
        "proposition_provenance",
        "propositions",
        "questions",
        "session_agents",
        "session_messages",
        "sessions",
        "sources",
        "sqlite_sequence",
    ];
    assert_eq!(tables(store.connection()), expected);
}

#[test]
fn an_n1_store_upgrades_to_the_latest_keeping_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noesis.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         INSERT INTO meta VALUES ('schema_version', '1');",
    )
    .unwrap();
    conn.execute_batch(schema::MIGRATIONS[0]).unwrap();
    add_session(&conn, "ns_1");
    drop(conn);
    let store = NoesisStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let conn = store.connection();
    for table in ["concepts", "evidence", "concept_state", "evidence_bundles"] {
        assert!(tables(conn).contains(&table.to_string()), "{table}");
    }
    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sessions, 1);
}

/// An N3 store as the binary before N4 left it: a session handed to its
/// Teacher, one in practice, and a question of each step it could infer.
fn n3_store(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         INSERT INTO meta VALUES ('schema_version', '3');",
    )
    .unwrap();
    for migration in &schema::MIGRATIONS[..3] {
        conn.execute_batch(migration).unwrap();
    }
    conn.execute_batch(
        "INSERT INTO sessions(id, goal, mode, phase, agent, created_at, updated_at) VALUES
             ('ns_t', 'learn', 'teach', 'teach', 'agt_teacher', 't0', 't0'),
             ('ns_p', 'practise', 'practice', 'practice', 'agt_practitioner', 't0', 't0');
         INSERT INTO questions(id, session, kind, prompt, collapsed, plan_node, answer_key, posed_at,
                               answer, answered_at) VALUES
             ('q_probe', 'ns_t', 'choice', 'p', 0, NULL, '[\"a\"]', 't1', '{}', 't1'),
             ('q_predict', 'ns_t', 'choice', 'p', 0, 'a', '[\"a\"]', 't2', '{}', 't2'),
             ('q_check', 'ns_t', 'recall', 'p', 1, 'a', '[\"a\"]', 't3', '{}', 't3'),
             ('q_goal', 'ns_t', 'goal', 'p', 0, NULL, '[]', 't4', NULL, NULL),
             ('q_practice', 'ns_p', 'recall', 'p', 0, NULL, '[\"a\"]', 't1', NULL, NULL);",
    )
    .unwrap();
}

#[test]
fn an_n3_store_upgrades_keeping_each_session_s_agent_and_inferring_each_step() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noesis.db");
    n3_store(&path);
    let store = NoesisStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 4);
    let conn = store.connection();
    let mut statement = conn
        .prepare("SELECT session, ord, agent, role, internal FROM session_agents ORDER BY session")
        .unwrap();
    let agents: Vec<(String, i64, String, String, String)> = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let expected = [
        ("ns_p", 0, "agt_practitioner", "practitioner", "[]"),
        ("ns_t", 0, "agt_teacher", "teacher", "[]"),
    ];
    let expected: Vec<_> = expected
        .iter()
        .map(|(s, o, a, r, i)| {
            (
                s.to_string(),
                *o,
                a.to_string(),
                r.to_string(),
                i.to_string(),
            )
        })
        .collect();
    assert_eq!(agents, expected);
    let step = |id: &str| -> String {
        conn.query_row("SELECT step FROM questions WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .unwrap()
    };
    let steps =
        ["probe", "predict", "check", "goal", "practice"].map(|id| step(&format!("q_{id}")));
    assert_eq!(steps, ["measure", "predict", "check", "goal", "practice"]);
    let stalled: Option<String> = conn
        .query_row(
            "SELECT stalled FROM sessions WHERE id = 'ns_t'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stalled, None, "no session starts stalled");
}

#[test]
fn a_session_lists_each_agent_once_and_only_known_roles_and_steps() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    add_session(conn, "ns_1");
    let add = |ord: i64, agent: &str, role: &str| {
        conn.execute(
            "INSERT INTO session_agents(session, ord, agent, role, started_at)
             VALUES ('ns_1', ?1, ?2, ?3, 't0')",
            rusqlite::params![ord, agent, role],
        )
    };
    add(0, "agt_1", "prober").unwrap();
    add(1, "agt_2", "teacher").unwrap();
    assert!(add(2, "agt_1", "teacher").is_err(), "an agent ran twice");
    assert!(
        add(2, "agt_3", "extractor").is_err(),
        "a role no session has"
    );
    let step = conn.execute(
        "INSERT INTO questions (id, session, kind, prompt, answer_key, posed_at, step)
         VALUES ('q_1', 'ns_1', 'recall', 'why?', '[]', 't1', 'lecture')",
        [],
    );
    assert!(step.is_err(), "a step no question is");
    conn.execute("DELETE FROM sessions WHERE id = 'ns_1'", [])
        .unwrap();
    let left: i64 = conn
        .query_row("SELECT count(*) FROM session_agents", [], |row| row.get(0))
        .unwrap();
    assert_eq!(left, 0, "a session's agents go with it");
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

#[cfg(unix)]
#[test]
fn reopening_a_permissive_store_makes_it_and_its_sidecars_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noesis.db");
    NoesisStore::open(&path).unwrap().close().unwrap();
    let wal = dir.path().join("noesis.db-wal");
    std::fs::write(&wal, b"").unwrap();
    for file in [&path, &wal] {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let store = NoesisStore::open(&path).unwrap();
    store
        .connection()
        .execute(
            "INSERT INTO sources VALUES ('s_1', 'notes', 'art_1', 't')",
            [],
        )
        .unwrap();
    let mode = |name: &str| {
        let metadata = std::fs::metadata(dir.path().join(name)).unwrap();
        metadata.permissions().mode() & 0o777
    };
    for name in ["noesis.db", "noesis.db-wal", "noesis.db-shm"] {
        assert_eq!(mode(name), 0o600, "{name}");
    }
    store.close().unwrap();
}

#[cfg(unix)]
#[test]
fn a_symlink_in_place_of_the_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere.db");
    std::fs::write(&target, b"").unwrap();
    let path = dir.path().join("noesis.db");
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let error = NoesisStore::open(&path).unwrap_err();
    assert!(matches!(error, StoreError::Io { .. }), "{error}");
    assert!(error.to_string().contains("symlink"), "{error}");
}

/// One row in every N1 and N2 table, through their keys and checks.
const N2_ROWS: &str = r#"
INSERT INTO sessions VALUES ('ns_1', 'learn rust', 'teach', 'teach', 'agt_1', 't0', 't1');
INSERT INTO session_messages VALUES ('ns_1', 0, 'm1', 'tutor', 12, 't0');
INSERT INTO plans VALUES ('ns_1', 1, '{"nodes":[],"edges":[]}', 't0');
INSERT INTO questions (id, session, kind, prompt, answer_key, posed_at, answer, correct, answered_at)
    VALUES ('q_1', 'ns_1', 'recall', 'Why?', '"because"', 't0', '{"text":"x"}', 1, 't1');
INSERT INTO problems (id, session, statement, mode, solution, posed_at)
    VALUES ('pr_1', 'ns_1', 'Solve it', 'recall', 'the solution', 't0');
INSERT INTO sources VALUES ('s_1', 'notes', 'art_1', 't0');
INSERT INTO propositions VALUES ('p_1', 'Ownership', 'Each value has one owner.', 't0');
INSERT INTO propositions VALUES ('p_2', 'Drop', 'A value is dropped with its owner.', 't0');
INSERT INTO proposition_aliases VALUES ('p_1', 'owner rule');
INSERT INTO proposition_provenance VALUES ('p_1', 'ns_1', 'm1');
INSERT INTO chains VALUES ('c_1', 'p_2', 't0');
INSERT INTO chain_premises VALUES ('c_1', 'p_1');
INSERT INTO chain_provenance VALUES ('c_1', 'ns_1', 'm1');
INSERT INTO clusters VALUES ('cl_1', 'ns_1');
INSERT INTO cluster_members VALUES ('cl_1', 'p_1');
INSERT INTO coverage VALUES ('ns_1', 'm1', printf('%064d', 0), 12, 't0');
INSERT INTO jobs (id, kind, target, state, created_at, updated_at)
    VALUES ('j_1', 'extract_session', 'ns_1', 'succeeded', 't0', 't1');
INSERT INTO concepts VALUES ('k_1', 'proposition', 'p_1', 'Ownership', 't0');
INSERT INTO evidence (concept, session, at, kind, outcome, confidence)
    VALUES ('k_1', 'ns_1', 't0', 'check_answer', 'correct', 70);
INSERT INTO concept_state (concept, state, strength, as_of_evidence, derived_at)
    VALUES ('k_1', 'exposed', 0.4, 1, 't1');
"#;

/// Every row of every table, in a stable order, for comparing two stores.
fn dump(conn: &Connection) -> Vec<(String, Vec<Vec<rusqlite::types::Value>>)> {
    let mut out = Vec::new();
    for table in tables(conn) {
        let sql = format!("SELECT * FROM \"{table}\" ORDER BY 1, 2");
        let mut statement = conn.prepare(&sql).unwrap();
        let width = statement.column_count();
        let rows = statement
            .query_map([], |row| (0..width).map(|i| row.get(i)).collect())
            .unwrap();
        out.push((table, rows.collect::<Result<_, _>>().unwrap()));
    }
    out
}

#[test]
fn an_n2_store_upgrades_to_n3_keeping_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noesis.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         INSERT INTO meta VALUES ('schema_version', '2');",
    )
    .unwrap();
    conn.execute_batch(schema::MIGRATIONS[0]).unwrap();
    conn.execute_batch(schema::MIGRATIONS[1]).unwrap();
    conn.execute_batch(N2_ROWS).unwrap();
    let before = dump(&conn);
    let empty: Vec<_> = before
        .iter()
        .filter(|(table, rows)| rows.is_empty() && table != "sqlite_sequence")
        .map(|(table, _)| table.as_str())
        .collect();
    assert!(
        empty.is_empty(),
        "every N1 and N2 table holds a row: {empty:?}"
    );
    drop(conn);

    let store = NoesisStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 3);
    let after = dump(store.connection());
    let unchanged = |(table, _): &&(String, Vec<Vec<rusqlite::types::Value>>)| {
        !matches!(
            table.as_str(),
            "meta" | "evidence_bundles" | "evidence_items"
        )
    };
    assert_eq!(
        after.iter().filter(unchanged).collect::<Vec<_>>(),
        before.iter().filter(unchanged).collect::<Vec<_>>(),
        "N3 keeps every N1 and N2 row as it was"
    );
    for table in ["evidence_bundles", "evidence_items"] {
        let rows = &after.iter().find(|(t, _)| t == table).unwrap().1;
        assert!(rows.is_empty(), "{table} starts empty");
    }
    store.close().unwrap();
}
