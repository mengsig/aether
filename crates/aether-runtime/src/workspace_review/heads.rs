//! The commit an agent's workspace had checked out when each of its turns
//! started and ended, so the commits in between can be put down to it.

use std::collections::BTreeMap;

use aether_core::agent::Agent;
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_store::{Commit, EventFilter};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::workspace::review;
use crate::{Result, Runtime, RuntimeError};

const EVENT: &str = "workspace.head";
const PAGE: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Boundary {
    Start,
    End,
}

#[derive(Debug, Serialize, Deserialize)]
struct HeadRecord {
    turn: u32,
    boundary: Boundary,
    head: String,
}

/// One turn's recorded HEADs; `end` is absent while it runs or after a crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnWindow {
    pub turn: u32,
    pub start: String,
    pub end: Option<String>,
}

/// Record the workspace HEAD at a turn boundary and return it. An unchanged
/// end is recorded too, so the turn is closed. A workspace outside Git records
/// nothing, and a Git failure only costs attribution.
pub(crate) async fn record_turn_head(
    runtime: &Runtime,
    agent: &Agent,
    turn: u32,
    boundary: Boundary,
) -> Result<Option<String>> {
    debug_assert!(turn > 0, "turns count from one");
    let Some(root) = agent.scope.workspace_root.clone() else {
        return Ok(None);
    };
    let read = tokio::task::spawn_blocking(move || review::head(&root))
        .await
        .map_err(|e| RuntimeError::Internal(format!("workspace HEAD read failed: {e}")))?;
    let head = match read {
        Ok(Some(head)) => head,
        Ok(None) => return Ok(None),
        Err(error) => {
            tracing::warn!(agent = %agent.id, %error, "cannot read the workspace HEAD; this turn's commits stay unattributed");
            return Ok(None);
        }
    };
    let record = HeadRecord {
        turn,
        boundary,
        head,
    };
    runtime.apply(
        Commit::new().event(
            EventDraft::new(
                Actor::System,
                EventKind::Custom {
                    name: EVENT.into(),
                    payload: json!(record),
                },
            )
            .agent(agent.id)
            .parent(agent.parent)
            .run(agent.run),
        ),
    )?;
    Ok(Some(record.head))
}

/// Every turn of `agent` whose start HEAD was recorded, in turn order.
pub(crate) fn turn_windows(runtime: &Runtime, agent: &Agent) -> Result<Vec<TurnWindow>> {
    let filter = EventFilter {
        types: vec!["custom".into()],
        ..EventFilter::own_events(agent.id)
    };
    let mut windows: BTreeMap<u32, TurnWindow> = BTreeMap::new();
    let mut after = 0;
    loop {
        let page = runtime.store.events(after, PAGE, &filter)?;
        for event in &page {
            let EventKind::Custom { name, payload } = &event.kind else {
                continue;
            };
            if name != EVENT {
                continue;
            }
            let record: HeadRecord = serde_json::from_value(payload.clone()).map_err(|e| {
                RuntimeError::Internal(format!("malformed {EVENT} event {}: {e}", event.seq))
            })?;
            add(&mut windows, record);
        }
        match page.last() {
            Some(last) if page.len() == PAGE => after = last.seq,
            _ => break,
        }
    }
    Ok(windows.into_values().collect())
}

fn add(windows: &mut BTreeMap<u32, TurnWindow>, record: HeadRecord) {
    match record.boundary {
        Boundary::Start => {
            windows.insert(
                record.turn,
                TurnWindow {
                    turn: record.turn,
                    start: record.head,
                    end: None,
                },
            );
        }
        Boundary::End => {
            if let Some(window) = windows.get_mut(&record.turn) {
                window.end = Some(record.head);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(turn: u32, boundary: Boundary, head: &str) -> HeadRecord {
        HeadRecord {
            turn,
            boundary,
            head: head.into(),
        }
    }

    #[test]
    fn an_end_closes_its_own_turn_and_an_orphan_end_is_dropped() {
        let mut windows = BTreeMap::new();
        add(&mut windows, record(2, Boundary::Start, "b"));
        add(&mut windows, record(1, Boundary::Start, "a"));
        add(&mut windows, record(1, Boundary::End, "a2"));
        add(&mut windows, record(3, Boundary::End, "orphan"));
        let windows: Vec<_> = windows.into_values().collect();
        assert_eq!(
            windows,
            vec![
                TurnWindow {
                    turn: 1,
                    start: "a".into(),
                    end: Some("a2".into())
                },
                TurnWindow {
                    turn: 2,
                    start: "b".into(),
                    end: None
                },
            ]
        );
    }
}
