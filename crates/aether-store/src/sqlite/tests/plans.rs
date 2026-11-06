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
    // The live agents alone are sought by status and sorted, see below.
    let mut args = Vec::new();
    let steps = plan(
        &store,
        &agent_list_sql(&AgentFilter::default(), &mut args),
        &args,
    );
    assert!(
        steps.iter().any(|s| s.2.contains("INDEX agents_created")),
        "{steps:?}"
    );
    assert!(
        !steps.iter().any(|s| s.2.contains("TEMP B-TREE")),
        "{steps:?}"
    );
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

/// A store where `run` billed `own` usage receipts among `others` of other runs.
fn receipts_among_others(own: usize, others: usize) -> (SqliteStore, RunId) {
    let store = SqliteStore::open_in_memory().expect("open");
    let (run, busy) = (RunId::new(), RunId::new());
    let receipt = |run: RunId| {
        let usage = aether_core::model::Usage::default();
        EventDraft::new(
            Actor::System,
            EventKind::AgentUsageUpdated {
                total: usage,
                turn_usage: usage,
            },
        )
        .agent(AgentId::new())
        .run(run)
    };
    let noise = (0..others).fold(Commit::new(), |commit, _| commit.event(receipt(busy)));
    store.commit(noise).expect("commit other runs");
    let mine = (0..own).fold(Commit::new(), |commit, _| commit.event(receipt(run)));
    store.commit(mine).expect("commit the run's receipts");
    (store, run)
}

/// Virtual-machine steps to read `run`'s newest page of usage receipts.
fn run_receipt_page_work(store: &SqliteStore, run: RunId) -> (usize, i32) {
    let filter = EventFilter {
        run: Some(run),
        types: vec!["agent.usage_updated".into()],
        ..Default::default()
    };
    let mut args = Vec::new();
    let predicate = crate::sqlite::event_queries::filter_predicate(&filter, &mut args);
    let sql =
        crate::sqlite::event_queries::page_sql("seq", &predicate, 0, None, 256, false, &mut args);
    let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|arg| arg.as_ref()).collect();
    let conn = store.conn.lock();
    let mut statement = conn.prepare(&sql).expect("prepare");
    let rows = statement
        .query_map(refs.as_slice(), |r| r.get::<_, i64>(0))
        .expect("query")
        .count();
    (rows, statement.get_status(StatementStatus::VmStep))
}

#[test]
fn a_runs_receipt_page_does_not_walk_every_other_runs_receipts() {
    // `status` reads this page for every live run (#333 follow-up): walked
    // through the type index, it read every receipt the store ever kept.
    let (quiet, quiet_run) = receipts_among_others(15, 100);
    let (busy, busy_run) = receipts_among_others(15, 4_000);
    let (quiet_rows, quiet_steps) = run_receipt_page_work(&quiet, quiet_run);
    let (rows, steps) = run_receipt_page_work(&busy, busy_run);
    assert_eq!((quiet_rows, rows), (15, 15), "every receipt of the run");
    assert!(
        steps < quiet_steps * 2,
        "other runs' receipts cost {steps} steps, against {quiet_steps} without them"
    );
}

#[test]
fn the_live_agents_are_sought_by_status_not_scanned() {
    // `status` lists them on every poll: `NOT IN` over the terminal statuses
    // walked every agent the store ever kept. Sorting the few live ones is cheap.
    let store = SqliteStore::open_in_memory().expect("open");
    let live = AgentFilter {
        live_only: true,
        ..Default::default()
    };
    let blocked = AgentFilter {
        statuses: vec![aether_core::agent::AgentStatusKind::Blocked],
        ..live.clone()
    };
    for filter in [live, blocked] {
        let mut args = Vec::new();
        let steps = plan(&store, &agent_list_sql(&filter, &mut args), &args);
        assert!(
            steps
                .iter()
                .any(|s| s.2.contains("INDEX agents_status (status=?)")),
            "{steps:?}"
        );
        assert!(
            !steps.iter().any(|s| s.2.starts_with("SCAN agents")),
            "{steps:?}"
        );
    }
}

#[test]
fn a_runs_receipts_are_read_from_the_run_and_type_index_in_order() {
    let store = SqliteStore::open_in_memory().expect("open");
    let filter = EventFilter {
        run: Some(RunId::new()),
        types: vec!["agent.usage_updated".into()],
        ..Default::default()
    };
    for (ascending, before) in [(false, None), (false, Some(900)), (true, None)] {
        let mut args = Vec::new();
        let predicate = crate::sqlite::event_queries::filter_predicate(&filter, &mut args);
        let sql = crate::sqlite::event_queries::page_sql(
            "seq, body",
            &predicate,
            0,
            before,
            256,
            ascending,
            &mut args,
        );
        let steps = plan(&store, &sql, &args);
        assert!(
            steps
                .iter()
                .any(|s| s.2.contains("INDEX events_run_type (run=? AND type=?")),
            "{steps:?}"
        );
        assert!(
            !steps.iter().any(|s| s.2.contains("TEMP B-TREE")),
            "{steps:?}"
        );
    }
}

fn no_scan_or_sort(steps: &[(i64, i64, String)]) {
    assert!(
        steps
            .iter()
            .all(|s| !s.2.starts_with("SCAN a") && !s.2.starts_with("SCAN agents")),
        "{steps:?}"
    );
    assert!(
        steps.iter().all(|s| !s.2.contains("TEMP B-TREE")),
        "{steps:?}"
    );
}

#[test]
fn the_fleet_and_run_totals_are_one_row_read_by_key() {
    use crate::sqlite::usage_totals::TOTALS_COLUMNS;
    let store = SqliteStore::open_in_memory().expect("open");
    let args: Arguments = vec![Box::new(String::new())];
    let sql = format!("SELECT {TOTALS_COLUMNS} FROM usage_totals WHERE scope = ?1");
    let steps = plan(&store, &sql, &args);
    assert_eq!(
        steps.iter().map(|s| s.2.as_str()).collect::<Vec<_>>(),
        ["SEARCH usage_totals USING PRIMARY KEY (scope=?)"]
    );
    let args: Arguments = vec![Box::new(RunId::new().to_string())];
    let steps = plan(
        &store,
        "SELECT MIN(created_at) FROM agent_usage WHERE run = ?1",
        &args,
    );
    assert_eq!(
        steps.iter().map(|s| s.2.as_str()).collect::<Vec<_>>(),
        ["SEARCH agent_usage USING COVERING INDEX agent_usage_run (run=?)"]
    );
}

#[test]
fn a_members_usage_is_read_by_key() {
    use crate::sqlite::usage_totals::USAGE_COLUMNS;
    let store = SqliteStore::open_in_memory().expect("open");
    let args: Arguments = vec![Box::new(AgentId::new().to_string())];
    let sql =
        format!("SELECT u.agent_id, {USAGE_COLUMNS} FROM agent_usage u WHERE u.agent_id = ?1");
    let steps = plan(&store, &sql, &args);
    assert_eq!(
        steps.iter().map(|s| s.2.as_str()).collect::<Vec<_>>(),
        ["SEARCH u USING PRIMARY KEY (agent_id=?)"]
    );
}

fn tree_query(
    run: Option<RunId>,
    root: Option<AgentId>,
    include_archived: bool,
) -> crate::TreeQuery {
    crate::TreeQuery {
        run,
        root,
        include_archived,
        role_prefix: None,
        after: Some(AgentId::new()),
        limit: crate::TREE_PAGE_MAX,
    }
}

fn boxed(args: Vec<String>) -> Arguments {
    args.into_iter()
        .map(|arg| Box::new(arg) as Box<dyn rusqlite::ToSql>)
        .collect()
}

#[test]
fn a_tree_page_seeks_live_agents_by_status_and_finished_ones_by_index_order() {
    use crate::sqlite::tree_pages::page_sql;
    let store = SqliteStore::open_in_memory().expect("open");
    let cases = [
        (None, "agents_archived_id (archived=? AND id<?)"),
        (
            Some(RunId::new()),
            "agents_run_archived_id (run=? AND archived=? AND id<?)",
        ),
    ];
    for (run, finished_index) in cases {
        let query = tree_query(run, None, false);
        let (pinned, finished, args) = page_sql(&query);
        let pinned_args = boxed(args[..args.len() - 1].to_vec());
        let steps = plan(&store, &pinned, &pinned_args);
        assert!(
            steps
                .iter()
                .any(|s| s.2 == "SEARCH a USING INDEX agents_status (status=?)"),
            "{steps:?}"
        );
        no_scan_or_sort(&steps);
        let steps = plan(&store, &finished, &boxed(args));
        assert!(
            steps
                .iter()
                .any(|s| s.2 == format!("SEARCH a USING INDEX {finished_index}")),
            "{steps:?}"
        );
        no_scan_or_sort(&steps);
    }
    let query = tree_query(Some(RunId::new()), None, true);
    let (_, finished, args) = page_sql(&query);
    let steps = plan(&store, &finished, &boxed(args));
    assert!(
        steps
            .iter()
            .any(|s| s.2 == "SEARCH a USING INDEX agents_run_id (run=? AND id<?)"),
        "{steps:?}"
    );
    no_scan_or_sort(&steps);
}

#[test]
fn a_rooted_tree_page_walks_the_subtree_down_the_closure_key() {
    use crate::sqlite::tree_pages::{ancestors_sql, page_sql};
    let store = SqliteStore::open_in_memory().expect("open");
    let query = tree_query(None, Some(AgentId::new()), false);
    let (pinned, walk, args) = page_sql(&query);
    let steps = plan(&store, &walk, &boxed(args.clone()));
    assert!(
        steps
            .iter()
            .any(|s| s.2 == "SEARCH t USING PRIMARY KEY (ancestor=? AND descendant<?)"),
        "{steps:?}"
    );
    no_scan_or_sort(&steps);
    assert!(
        steps.iter().all(|s| !s.2.starts_with("SCAN t")),
        "{steps:?}"
    );
    let steps = plan(&store, &pinned, &boxed(args[..args.len() - 1].to_vec()));
    assert!(
        steps
            .iter()
            .any(|s| s.2 == "SEARCH a USING INDEX agents_status (status=?)"),
        "{steps:?}"
    );
    assert!(
        steps.iter().any(|s| s.2.starts_with("SEARCH s ")
            && s.2.contains("descendant=?")
            && s.2.contains("ancestor=?")),
        "the root is checked by key, not by walking its subtree: {steps:?}"
    );
    no_scan_or_sort(&steps);
    let (above, mut args) = ancestors_sql(&query);
    args.push("[]".to_owned());
    let steps = plan(&store, &above, &boxed(args));
    assert!(
        steps
            .iter()
            .any(|s| s.2 == "SEARCH c USING INDEX sqlite_autoindex_agents_1 (id=?)"),
        "{steps:?}"
    );
    no_scan_or_sort(&steps);
}

#[test]
fn a_subtree_roll_up_reads_its_totals_by_key() {
    use crate::sqlite::tree_pages::subtree_totals_sql;
    let store = SqliteStore::open_in_memory().expect("open");
    let id = AgentId::new().to_string();
    let children = serde_json::to_string(&[AgentId::new().to_string()]).unwrap();
    for run in [None, Some(RunId::new().to_string())] {
        let mut args = vec![id.clone(), children.clone()];
        args.extend(run.clone());
        let steps = plan(
            &store,
            &subtree_totals_sql(true, run.is_some()),
            &boxed(args),
        );
        let by_scope = if run.is_some() {
            "(scope=?)"
        } else {
            "(scope>? AND scope<?)"
        };
        let seeks = steps
            .iter()
            .filter(|s| s.2 == format!("SEARCH usage_totals USING PRIMARY KEY {by_scope}"))
            .count();
        assert_eq!(
            seeks, 2,
            "the agent's subtree and each child's, by key: {steps:?}"
        );
        assert!(
            steps
                .iter()
                .any(|s| s.2.starts_with("SEARCH u USING PRIMARY KEY (agent_id=?)")),
            "{steps:?}"
        );
        assert!(
            steps.iter().all(|s| !s.2.starts_with("SCAN usage_totals")),
            "{steps:?}"
        );
    }
}

/// Virtual-machine steps to read a run's receipt page of 256, newest first.
fn receipt_page_steps(store: &SqliteStore, run: RunId) -> i32 {
    run_receipt_page_work(store, run).1
}

#[test]
fn a_runs_receipt_page_costs_the_same_however_long_its_other_history() {
    // A run alive for days writes far more events than receipts: walking its
    // events for 256 receipts read its whole history (#414 follow-up).
    let lived = |filler: usize| {
        let (store, run) = receipts_among_others(15, 1);
        let noise = (0..filler).fold(Commit::new(), |commit, _| {
            commit.event(
                EventDraft::new(Actor::System, EventKind::AgentReady)
                    .agent(AgentId::new())
                    .run(run),
            )
        });
        store.commit(noise).expect("the run's other events");
        (store, run)
    };
    let (short, short_run) = lived(100);
    let (long, long_run) = lived(5_000);
    let (short_steps, steps) = (
        receipt_page_steps(&short, short_run),
        receipt_page_steps(&long, long_run),
    );
    assert!(steps > 0 && short_steps > 0, "both pages ran");
    assert!(
        steps < short_steps * 2,
        "the run's other events cost {steps} steps, against {short_steps} with few"
    );
}

#[test]
fn the_active_tasks_and_pending_approvals_are_counted_by_index() {
    use crate::sqlite::fleet_reads::{COUNT_PENDING_APPROVALS_SQL, COUNT_TASKS_SQL};
    let store = SqliteStore::open_in_memory().expect("open");
    let args = boxed(vec![r#"["active","assigned"]"#.to_owned()]);
    let steps = plan(&store, COUNT_TASKS_SQL, &args);
    assert!(
        steps
            .iter()
            .any(|s| s.2 == "SEARCH t USING INDEX tasks_status (status=?)"),
        "{steps:?}"
    );
    assert!(
        steps
            .iter()
            .any(|s| s.2.starts_with("SEARCH m USING") && s.2.contains("(key=?)")),
        "{steps:?}"
    );
    assert!(
        steps.iter().all(|s| !s.2.starts_with("SCAN t")),
        "{steps:?}"
    );
    let steps = plan(&store, COUNT_PENDING_APPROVALS_SQL, &Vec::new());
    assert_eq!(
        steps.iter().map(|s| s.2.as_str()).collect::<Vec<_>>(),
        ["SEARCH approvals USING COVERING INDEX approvals_state (state=?)"]
    );
}
