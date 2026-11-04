//! The learner tables stand apart from the proposition graph, `evidence` is
//! append-only, and only the rebuild writes `concept_state`.

use std::path::{Path, PathBuf};

use aether_core::noesis::ConceptKind;
use rusqlite::Connection;

use super::store_rules::{append_each, open_temp};
use super::*;
use crate::learner::{Selection, ensure_concept, states};

const LEARNER_TABLES: [&str; 3] = ["concepts", "evidence", "concept_state"];
const GRAPH_TABLES: [&str; 10] = [
    "propositions",
    "proposition_aliases",
    "proposition_provenance",
    "chains",
    "chain_premises",
    "chain_provenance",
    "clusters",
    "cluster_members",
    "coverage",
    "sources",
];

fn names(conn: &Connection, sql: &str) -> Vec<(String, String)> {
    let mut statement = conn.prepare(sql).unwrap();
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

fn mentions(text: &str, table: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|word| word.eq_ignore_ascii_case(table))
}

#[test]
fn no_key_trigger_or_view_joins_the_learner_tables_to_the_graph() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    let tables = names(
        conn,
        "SELECT name, name FROM sqlite_master WHERE type = 'table'",
    );
    for table in LEARNER_TABLES.iter().chain(&GRAPH_TABLES) {
        assert!(
            tables.iter().any(|(name, _)| name == table),
            "{table} exists"
        );
    }
    for (table, _) in &tables {
        let sql = format!("SELECT \"table\", \"from\" FROM pragma_foreign_key_list('{table}')");
        for (target, column) in names(conn, &sql) {
            let crosses = LEARNER_TABLES.contains(&table.as_str())
                != LEARNER_TABLES.contains(&target.as_str());
            assert!(!crosses, "{table}.{column} references {target}");
        }
    }
    let code = names(
        conn,
        "SELECT name, sql FROM sqlite_master WHERE type IN ('trigger', 'view')",
    );
    for (name, sql) in code {
        let learner = LEARNER_TABLES.iter().any(|t| mentions(&sql, t));
        let graph = GRAPH_TABLES.iter().any(|t| mentions(&sql, t));
        assert!(
            !(learner && graph),
            "{name} joins the learner model and the graph"
        );
    }
}

#[test]
fn deleting_the_graph_leaves_the_learner_model_intact() {
    let (_dir, store) = open_temp();
    let conn = store.connection();
    conn.execute_batch(
        "INSERT INTO propositions VALUES ('p_1', 'speed', 'speed is distance over time', 't0');
         INSERT INTO propositions VALUES ('p_2', 'velocity', 'velocity has direction', 't0');
         INSERT INTO chains VALUES ('c_1', 'p_2', 't0');
         INSERT INTO chain_premises VALUES ('c_1', 'p_1');",
    )
    .unwrap();
    let concept =
        ensure_concept(conn, ConceptKind::Proposition, "p_1", "speed", day(1, 0)).unwrap();
    append_each(
        conn,
        &concept.id,
        &[
            exposure(day(1, 0)),
            check(EvidenceOutcome::Correct, Some(90), day(1, 1)),
        ],
    );
    let before = states(conn, &Selection::default(), day(2, 0)).unwrap();
    for table in GRAPH_TABLES {
        conn.execute(&format!("DELETE FROM {table}"), []).unwrap();
    }
    let propositions: i64 = conn
        .query_row("SELECT count(*) FROM propositions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(propositions, 0);
    let evidence: i64 = conn
        .query_row("SELECT count(*) FROM evidence", [], |row| row.get(0))
        .unwrap();
    assert_eq!(evidence, 2);
    assert_eq!(
        states(conn, &Selection::default(), day(2, 0)).unwrap(),
        before
    );
    assert_eq!(
        before[0].concept.reference, "p_1",
        "the id stays, a plain value"
    );
}

/// This crate's Rust sources, test files included.
fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(sources(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
    found
}

/// Words of `text`, lowercased, so SQL split over lines still reads in order.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn statements_on(words: &[String], verbs: &[&[&str]], table: &str) -> usize {
    let mut count = 0;
    for verb in verbs {
        for window in words.windows(verb.len() + 1) {
            let (head, target) = window.split_at(verb.len());
            if head.iter().zip(*verb).all(|(w, v)| w == v) && target[0] == table {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn no_statement_updates_or_deletes_evidence_and_only_the_rebuild_writes_the_cache() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = sources(&root);
    assert!(files.len() > 10, "found the crate's sources: {files:?}");
    let edits: &[&[&str]] = &[
        &["update"],
        &["delete", "from"],
        &["replace", "into"],
        &["or", "replace", "into"],
    ];
    let inserts: &[&[&str]] = &[&["insert", "into"]];
    let mut cache_writers = Vec::new();
    // Tests may try an edit, to see the store refuse it.
    let is_test = |file: &Path| {
        file.components().any(|c| c.as_os_str() == "tests") || file.ends_with("tests.rs")
    };
    for file in files.iter().filter(|file| !is_test(file)) {
        let text = std::fs::read_to_string(file).unwrap();
        let words = words(&text);
        assert_eq!(
            statements_on(&words, edits, "evidence"),
            0,
            "{} edits evidence",
            file.display()
        );
        let writes = statements_on(&words, inserts, "concept_state")
            + statements_on(&words, edits, "concept_state");
        if writes > 0 {
            cache_writers.push((file.strip_prefix(&root).unwrap().to_path_buf(), writes));
        }
    }
    assert_eq!(cache_writers, [(PathBuf::from("learner/store.rs"), 1)]);
}

#[test]
fn learner_code_never_names_a_graph_table() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/learner");
    for file in std::fs::read_dir(&dir).unwrap() {
        let path = file.unwrap().path();
        if path.is_dir() {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for table in GRAPH_TABLES {
            assert!(!mentions(&text, table), "{} names {table}", path.display());
        }
    }
}
