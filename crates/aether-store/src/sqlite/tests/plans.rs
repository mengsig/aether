//! Hot reads must not sort whole tables or event bodies: a sort past ~2 MB
//! (the page cache) spills to an `etilqs_*` temp file on every call (#57).
use super::*;
use crate::sqlite::event_queries::{Arguments, family_page_sql};
use crate::sqlite::store::{agent_list_sql, task_list_sql};
use rusqlite::StatementStatus;

/// `(id, parent, detail)` for each step of `sql`'s plan.
fn plan(store: &SqliteStore, sql: &str, args: &Arguments) -> Vec<(i64, i64, String)> {
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|arg| arg.as_ref()).collect();
    let conn = store.conn.lock();
    let steps: Vec<(i64, i64, String)> = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("prepare")
        .query_map(refs.as_slice(), |r| Ok((r.get(0)?, r.get(1)?, r.get(3)?)))
        .expect("plan")
        .collect::<std::result::Result<_, _>>()
        .expect("rows");
    assert!(!steps.is_empty(), "a plan has steps: {sql}");
    steps
}

/// Whether `step` sits under a `LIST SUBQUERY`, the seq-only side of a merge.
fn under_subquery(steps: &[(i64, i64, String)], step: &(i64, i64, String)) -> bool {
    let mut parent = step.1;
    while parent != 0 {
        let Some(up) = steps.iter().find(|s| s.0 == parent) else {
            return false;
        };
        if up.2.starts_with("LIST SUBQUERY") {
            return true;
        }
        parent = up.1;
    }
    false
}

#[test]
fn full_agent_and_task_listings_stream_in_creation_order() {
    let store = SqliteStore::open_in_memory().expect("open");
    let live = AgentFilter {
        live_only: true,
        ..Default::default()
    };
    for filter in [AgentFilter::default(), live] {
        let mut args = Vec::new();
        let steps = plan(&store, &agent_list_sql(&filter, &mut args), &args);
        assert!(
            steps.iter().any(|s| s.2.contains("INDEX agents_created")),
            "{steps:?}"
        );
        assert!(
            !steps.iter().any(|s| s.2.contains("TEMP B-TREE")),
            "{steps:?}"
        );
    }
    let mut args = Vec::new();
    let steps = plan(
        &store,
        &task_list_sql(&TaskFilter::default(), &mut args),
        &args,
    );
    assert!(
        steps.iter().any(|s| s.2.contains("INDEX tasks_created")),
        "{steps:?}"
    );
    assert!(
        !steps.iter().any(|s| s.2.contains("TEMP B-TREE")),
        "{steps:?}"
    );
}

#[test]
fn an_agents_page_with_its_children_sorts_seqs_not_bodies() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut filter = EventFilter::for_agent(AgentId::new());
    for child_types in [vec![], vec!["agent.started".to_owned()]] {
        filter.child_types = child_types;
        for (ascending, before) in [(true, None), (false, Some(u64::MAX)), (false, Some(500))] {
            let mut args = Vec::new();
            let sql = family_page_sql(&filter, 100, before, 5_000, ascending, &mut args);
            let steps = plan(&store, &sql, &args);
            assert!(
                steps
                    .iter()
                    .any(|s| s.1 == 0 && s.2.contains("INTEGER PRIMARY KEY")),
                "bodies are read by seq: {steps:?}"
            );
            assert!(
                !steps.iter().any(|s| s.2.contains("AUTOMATIC")),
                "{steps:?}"
            );
            assert!(
                steps
                    .iter()
                    .filter(|s| s.2.contains("TEMP B-TREE"))
                    .all(|s| under_subquery(&steps, s)),
                "only the seq merge sorts: {steps:?}"
            );
        }
    }
}

#[test]
fn a_runs_agents_and_tasks_stream_in_creation_order() {
    let store = SqliteStore::open_in_memory().expect("open");
    let run = Some(RunId::new());
    for filter in [
        AgentFilter {
            run,
            ..Default::default()
        },
        AgentFilter {
            run,
            live_only: true,
            ..Default::default()
        },
    ] {
        let mut args = Vec::new();
        let steps = plan(&store, &agent_list_sql(&filter, &mut args), &args);
        assert!(
            steps
                .iter()
                .any(|s| s.2.contains("INDEX agents_run_created (run=?)")),
            "{steps:?}"
        );
        assert!(
            !steps.iter().any(|s| s.2.contains("TEMP B-TREE")),
            "{steps:?}"
        );
    }
    let mut args = Vec::new();
    let filter = TaskFilter {
        run,
        ..Default::default()
    };
    let steps = plan(&store, &task_list_sql(&filter, &mut args), &args);
    assert!(
        steps
            .iter()
            .any(|s| s.2.contains("INDEX tasks_run_created (run=?)")),
        "{steps:?}"
    );
    assert!(
        !steps.iter().any(|s| s.2.contains("TEMP B-TREE")),
        "{steps:?}"
    );
}

/// A store whose `parent` has `rounds` of one own event and two of a child's,
/// between `noise` unrelated events before and `noise` after.
fn sparse_family(rounds: usize, noise: usize) -> (SqliteStore, AgentId) {
    let store = SqliteStore::open_in_memory().expect("open");
    let (parent, child, stranger) = (AgentId::new(), AgentId::new(), AgentId::new());
    let ready = || EventDraft::new(Actor::System, EventKind::AgentReady);
    let done = || EventDraft::new(Actor::System, EventKind::AgentCompleted { summary: None });
    let unrelated = |store: &SqliteStore| {
        let commit = (0..noise).fold(Commit::new(), |commit, _| {
            commit.event(done().agent(stranger))
        });
        store.commit(commit).expect("commit noise");
    };
    unrelated(&store);
    for _ in 0..rounds {
        let commit = Commit::new()
            .event(ready().agent(parent))
            .event(ready().agent(child).parent(Some(parent)))
            .event(done().agent(child).parent(Some(parent)));
        store.commit(commit).expect("commit family");
    }
    unrelated(&store);
    (store, parent)
}

/// Rows and virtual-machine steps to read one family page of 10.
fn family_page_work(store: &SqliteStore, filter: &EventFilter, ascending: bool) -> (usize, i32) {
    let mut args = Vec::new();
    let sql = family_page_sql(filter, 0, None, 10, ascending, &mut args);
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|arg| arg.as_ref()).collect();
    let conn = store.conn.lock();
    let mut statement = conn.prepare(&sql).expect("prepare");
    let rows = statement
        .query_map(refs.as_slice(), |r| r.get::<_, String>(0))
        .expect("query")
        .count();
    (rows, statement.get_status(StatementStatus::VmStep))
}

#[test]
fn a_family_page_reads_as_much_for_a_long_history_as_for_a_short_one() {
    // Twenty times the history: an unbounded branch, or a walk of the seq
    // range instead of the agent indexes, would read about twenty times as much.
    let (short, short_parent) = sparse_family(20, 60);
    let (long, long_parent) = sparse_family(400, 1_200);
    let shapes = [
        (vec![], true),
        (vec![], false),
        (vec!["agent.ready".to_owned()], false),
    ];
    for (child_types, ascending) in shapes {
        let work = |store: &SqliteStore, parent: AgentId| {
            let mut filter = EventFilter::for_agent(parent);
            filter.child_types = child_types.clone();
            family_page_work(store, &filter, ascending)
        };
        let (short_rows, short_steps) = work(&short, short_parent);
        let (rows, steps) = work(&long, long_parent);
        assert_eq!((short_rows, rows), (10, 10), "full pages");
        assert!(
            steps < short_steps * 2,
            "{child_types:?} ascending={ascending}: {short_steps} steps, then {steps}"
        );
    }
}
