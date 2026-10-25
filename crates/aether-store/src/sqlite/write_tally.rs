//! Counts what commits write, by event type and by table, so the daemon can
//! say which kind of record is filling the disk (`aether status`).

use super::SqliteStore;
use crate::Result;
use aether_core::event::EventKind;
use aether_core::time::{Timestamp, now};
use aether_core::write_stats::{KindWrites, WriteStats};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::BTreeMap;

/// Prefix of a mutation's table in [`KindWrites::kind`], keeping tables apart
/// from event types.
const TABLE_PREFIX: &str = "table:";

/// How an event is counted: its type, with a custom event's own name so the
/// streamed `context.usage` readings do not hide among other customs.
pub(crate) fn event_kind(kind: &EventKind) -> String {
    match kind {
        EventKind::Custom { name, .. } => format!("custom:{name}"),
        other => other.type_name(),
    }
}

/// Rows and bytes one commit is about to write. Merged into the store's
/// ledger only after the transaction commits, so a rollback counts nothing.
#[derive(Debug, Default)]
pub(crate) struct WriteTally {
    kinds: BTreeMap<String, (u64, u64)>,
}

impl WriteTally {
    pub(super) fn add(&mut self, kind: &str, bytes: usize) {
        let entry = match self.kinds.get_mut(kind) {
            Some(entry) => entry,
            None => self.kinds.entry(kind.to_owned()).or_default(),
        };
        entry.0 += 1;
        entry.1 += bytes as u64;
    }

    /// The JSON body of a row of `table`, counted as written.
    pub(super) fn json<T: Serialize>(&mut self, table: &str, value: &T) -> Result<String> {
        let body = serde_json::to_string(value)?;
        self.row(table, body.len());
        Ok(body)
    }

    /// One row of `table` written with `bytes` of values.
    pub(super) fn row(&mut self, table: &str, bytes: usize) {
        debug_assert!(
            !table.is_empty() && !table.starts_with(TABLE_PREFIX),
            "{table}"
        );
        self.add(&format!("{TABLE_PREFIX}{table}"), bytes);
    }
}

/// Everything committed since the store opened.
#[derive(Debug)]
pub(super) struct WriteLedger {
    since: Timestamp,
    kinds: Mutex<BTreeMap<String, (u64, u64)>>,
}

impl WriteLedger {
    pub(super) fn new() -> Self {
        Self {
            since: now(),
            kinds: Mutex::default(),
        }
    }

    pub(super) fn merge(&self, tally: WriteTally) {
        let mut kinds = self.kinds.lock();
        for (kind, (rows, bytes)) in tally.kinds {
            let entry = kinds.entry(kind).or_default();
            entry.0 += rows;
            entry.1 += bytes;
        }
    }

    pub(super) fn snapshot(&self) -> WriteStats {
        let kinds: Vec<KindWrites> = self
            .kinds
            .lock()
            .iter()
            .map(|(kind, &(rows, bytes))| KindWrites {
                kind: kind.clone(),
                rows,
                bytes,
            })
            .collect();
        WriteStats {
            since: Some(self.since),
            total_rows: kinds.iter().map(|k| k.rows).sum(),
            total_bytes: kinds.iter().map(|k| k.bytes).sum(),
            kinds,
            db_bytes: None,
            wal_bytes: None,
        }
    }
}

impl SqliteStore {
    /// The ledger with the sizes of the files behind it.
    pub(super) fn write_stats_with_files(&self) -> WriteStats {
        let mut stats = self.writes.snapshot();
        if let Some(path) = &self.path {
            stats.db_bytes = std::fs::metadata(path).ok().map(|m| m.len());
            let mut wal = path.as_os_str().to_owned();
            wal.push("-wal");
            stats.wal_bytes = std::fs::metadata(wal).ok().map(|m| m.len());
        }
        stats
    }
}
