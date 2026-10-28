//! The rows written on the write connection while a history screen is open.
//! Expiry re-checks only those under the commit hold, not whole tables.

use parking_lot::Mutex;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Tables a history guard reads rows of. A write anywhere else cannot
/// change whether a run may expire.
pub(super) const WATCHED: &[&str] = &[
    "agents",
    "tasks",
    "events",
    "inbox",
    "snapshots",
    "invocations",
    "checkpoints",
    "approvals",
    "memories",
    "approval_rules",
    "leases",
    "schedules",
    "schedule_occurrences",
    "artifacts",
    "submissions",
    "meta",
];

/// Distinct rows remembered before the log gives up; every open watch then
/// reports its changes lost, and expiry falls back to the whole-table guards.
const MAX_ROWS: usize = 100_000;

#[derive(Debug, Default)]
struct State {
    seq: u64,
    /// The sequence of the latest committed write to each `(table, rowid)`.
    rows: HashMap<(usize, i64), u64>,
    /// The sequence at which the log last forgot rows; 0 when it never has.
    lost_at: u64,
    /// Each open watch by id, with the first sequence it must see.
    watches: BTreeMap<u64, u64>,
    next_watch: u64,
    /// Rows of the transaction open on the connection, or of the last one
    /// until the next writes: a watch opened mid-commit must still see them.
    recent: Vec<(usize, i64)>,
    recent_lost: bool,
    recent_committed: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ChangeLog {
    state: Mutex<State>,
}

impl ChangeLog {
    /// Record `conn`'s writes into `log` from now on.
    pub(super) fn install(log: &Arc<Self>, conn: &Connection) -> rusqlite::Result<()> {
        let written = log.clone();
        conn.update_hook(Some(move |action, database: &str, table: &str, rowid| {
            // A deleted row can no longer hold a reference.
            if action != rusqlite::hooks::Action::SQLITE_DELETE && database == "main" {
                written.written(table, rowid);
            }
        }))?;
        let committed = log.clone();
        conn.commit_hook(Some(move || {
            committed.committed();
            false
        }))?;
        let rolled_back = log.clone();
        conn.rollback_hook(Some(move || rolled_back.rolled_back()))
    }

    /// Whether a screen is open.
    pub(super) fn watching(&self) -> bool {
        !self.state.lock().watches.is_empty()
    }

    fn written(&self, table: &str, rowid: i64) {
        let Some(table) = WATCHED.iter().position(|watched| *watched == table) else {
            return;
        };
        let mut state = self.state.lock();
        if state.recent_committed {
            state.recent.clear();
            state.recent_lost = false;
            state.recent_committed = false;
        }
        if state.recent.len() >= MAX_ROWS {
            state.recent_lost = true;
            return;
        }
        state.recent.push((table, rowid));
    }

    fn committed(&self) {
        let mut state = self.state.lock();
        if !state.watches.is_empty() {
            state.seq += 1;
            let seq = state.seq;
            state.keep_recent(seq);
        }
        state.recent_committed = true;
    }

    fn rolled_back(&self) {
        let mut state = self.state.lock();
        state.recent.clear();
        state.recent_lost = false;
        state.recent_committed = false;
    }

    /// Start remembering commits. With a transaction `in_flight` on the
    /// connection, its rows count as written since: it may land unseen by a
    /// screen that starts now. Without one, the caller holds the connection.
    pub(super) fn watch(log: &Arc<Self>, in_flight: bool) -> Watch {
        let mut state = log.state.lock();
        state.seq += 1;
        let start = state.seq;
        if in_flight {
            state.keep_recent(start);
        }
        let id = state.next_watch;
        state.next_watch += 1;
        state.watches.insert(id, start);
        Watch {
            log: log.clone(),
            id,
            start,
        }
    }
}

impl State {
    fn keep_recent(&mut self, seq: u64) {
        let recent = std::mem::take(&mut self.recent);
        self.rows.extend(recent.iter().map(|row| (*row, seq)));
        self.recent = recent;
        if self.recent_lost || self.rows.len() > MAX_ROWS {
            self.rows.clear();
            self.lost_at = seq;
        }
        debug_assert!(self.rows.len() <= MAX_ROWS, "the log stays bounded");
    }
}

/// Writes since a moment, kept until dropped.
#[derive(Debug)]
pub(crate) struct Watch {
    log: Arc<ChangeLog>,
    id: u64,
    start: u64,
}

/// Row ids written per watched table, ascending.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes(BTreeMap<&'static str, Vec<i64>>);

impl Changes {
    pub(crate) fn rows(&self, table: &str) -> Option<&[i64]> {
        self.0.get(table).map(Vec::as_slice)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.values().map(Vec::len).sum()
    }
}

impl Watch {
    pub(crate) fn of(&self, log: &Arc<ChangeLog>) -> bool {
        Arc::ptr_eq(&self.log, log)
    }

    /// What was committed since the watch opened, or `None` when the log had
    /// to forget some of it.
    pub(crate) fn changes(&self) -> Option<Changes> {
        let state = self.log.state.lock();
        debug_assert!(
            state.watches.contains_key(&self.id),
            "a live watch is registered"
        );
        if state.lost_at >= self.start {
            return None;
        }
        let mut changes: BTreeMap<&'static str, Vec<i64>> = BTreeMap::new();
        for (&(table, rowid), &seq) in &state.rows {
            if seq >= self.start {
                changes.entry(WATCHED[table]).or_default().push(rowid);
            }
        }
        for rows in changes.values_mut() {
            rows.sort_unstable();
        }
        Some(Changes(changes))
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut state = self.log.state.lock();
        let removed = state.watches.remove(&self.id);
        debug_assert_eq!(removed, Some(self.start), "each watch is dropped once");
        match state.watches.values().min().copied() {
            None => state.rows.clear(),
            Some(oldest) => state.rows.retain(|_, seq| *seq >= oldest),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(log: &ChangeLog, rows: &[(&str, i64)]) {
        for (table, rowid) in rows {
            log.written(table, *rowid);
        }
        log.committed();
    }

    #[test]
    fn a_watch_sees_only_watched_commits_after_it_opened() {
        let log = Arc::new(ChangeLog::default());
        commit(&log, &[("inbox", 1)]);
        log.written("tasks", 4);
        log.rolled_back();
        let watch = ChangeLog::watch(&log, true);
        commit(
            &log,
            &[("inbox", 2), ("inbox", 2), ("expired_event_spans", 3)],
        );
        commit(&log, &[("agents", 9)]);
        let changes = watch.changes().expect("nothing lost");
        assert_eq!(changes.rows("inbox"), Some(&[2][..]));
        assert_eq!(changes.rows("agents"), Some(&[9][..]));
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn a_watch_opened_mid_commit_sees_that_commits_rows() {
        let log = Arc::new(ChangeLog::default());
        log.written("tasks", 1);
        let during = ChangeLog::watch(&log, true);
        log.committed();
        let after = ChangeLog::watch(&log, true);
        assert_eq!(
            during.changes().expect("kept").rows("tasks"),
            Some(&[1][..])
        );
        assert_eq!(after.changes().expect("kept").rows("tasks"), Some(&[1][..]));
        commit(&log, &[("tasks", 2)]);
        assert_eq!(
            after.changes().expect("kept").rows("tasks"),
            Some(&[1, 2][..])
        );
    }

    #[test]
    fn a_later_watch_keeps_an_earlier_ones_rows_and_the_last_drop_clears() {
        let log = Arc::new(ChangeLog::default());
        let early = ChangeLog::watch(&log, true);
        commit(&log, &[("tasks", 1)]);
        log.written("events", 7);
        let late = ChangeLog::watch(&log, true);
        log.committed();
        commit(&log, &[("tasks", 2)]);
        assert_eq!(late.changes().expect("kept").rows("tasks"), Some(&[2][..]));
        drop(late);
        let changes = early.changes().expect("kept");
        assert_eq!(changes.rows("tasks"), Some(&[1, 2][..]));
        assert_eq!(changes.rows("events"), Some(&[7][..]));
        drop(early);
        assert!(log.state.lock().rows.is_empty());
        commit(&log, &[("tasks", 3)]);
        assert!(
            log.state.lock().rows.is_empty(),
            "nothing is kept unwatched"
        );
    }

    #[test]
    fn overflow_reports_changes_lost_to_watches_open_at_the_time() {
        let log = Arc::new(ChangeLog::default());
        let watch = ChangeLog::watch(&log, true);
        for rowid in 0..=MAX_ROWS as i64 {
            log.written("inbox", rowid);
        }
        log.committed();
        assert_eq!(watch.changes(), None);
        let racing = ChangeLog::watch(&log, true);
        assert_eq!(
            racing.changes(),
            None,
            "the lost commit may still be landing"
        );
        commit(&log, &[("tasks", 5)]);
        let after = ChangeLog::watch(&log, true);
        commit(&log, &[("inbox", 1)]);
        assert_eq!(after.changes().expect("opened after").len(), 2);
    }
}
