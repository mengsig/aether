//! What the store has written since the daemon opened it, by kind of record.
//! Disk pressure from the daemon shows up here before it shows up anywhere else.

use serde::{Deserialize, Serialize};

use crate::time::Timestamp;

/// Rows and payload bytes written for one kind of record: an event type such
/// as `agent.usage_updated`, or a table such as `table:agents`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct KindWrites {
    pub kind: String,
    pub rows: u64,
    pub bytes: u64,
}

/// Payload bytes the store committed since `since`, heaviest kind first.
/// Counts JSON bodies, not SQLite pages: indexes and the WAL add on top, so
/// the file sizes are reported next to it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WriteStats {
    pub since: Option<Timestamp>,
    pub total_rows: u64,
    pub total_bytes: u64,
    pub kinds: Vec<KindWrites>,
    /// Size of the database file; absent for an in-memory store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_bytes: Option<u64>,
    /// Size of the write-ahead log beside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal_bytes: Option<u64>,
}

impl WriteStats {
    /// Keep the `limit` heaviest kinds and fold the rest into one `other` row,
    /// so a status reply stays small however many event types exist.
    pub fn top(mut self, limit: usize) -> Self {
        self.kinds
            .sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.kind.cmp(&b.kind)));
        if self.kinds.len() > limit {
            let rest = self.kinds.split_off(limit);
            self.kinds.push(KindWrites {
                kind: "other".into(),
                rows: rest.iter().map(|k| k.rows).sum(),
                bytes: rest.iter().map(|k| k.bytes).sum(),
            });
        }
        debug_assert!(self.kinds.len() <= limit + 1);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(name: &str, bytes: u64) -> KindWrites {
        KindWrites {
            kind: name.into(),
            rows: 1,
            bytes,
        }
    }

    #[test]
    fn top_keeps_the_heaviest_kinds_and_folds_the_rest() {
        let stats = WriteStats {
            total_rows: 4,
            total_bytes: 111,
            kinds: vec![kind("a", 1), kind("b", 100), kind("c", 6), kind("d", 4)],
            ..Default::default()
        }
        .top(2);
        let names: Vec<_> = stats.kinds.iter().map(|k| k.kind.as_str()).collect();
        assert_eq!(names, ["b", "c", "other"]);
        assert_eq!((stats.kinds[2].rows, stats.kinds[2].bytes), (2, 5));
        assert_eq!(stats.kinds.iter().map(|k| k.bytes).sum::<u64>(), 111);
    }
}
