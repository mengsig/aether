use super::retention::SubmissionSweep;
use super::*;
use crate::{
    AgentFilter, ArtifactFilter, Commit, EventFilter, ImportedEvent, MemoryQuery, Mutation, Store,
    StoreError, TaskFilter,
};
use aether_core::{
    agent::{Agent, AgentConfig, AgentStats, AgentStatus, Role, Scope},
    artifact::Artifact,
    event::{Actor, EventDraft, EventKind},
    id::{AgentId, ArtifactId, EventId, InboxItemId, InvocationId, MemoryId, RunId, ToolCallId},
    idempotency::{IdempotencyKey, RecordedOutcome},
    inbox::{InboxItem, InboxKind},
    lease::Lease,
    memory::{MemoryCandidate, MemoryKind, MemoryProvenance, MemoryRecord, MemoryScope},
    model::{ContentBlock, MessageRole, ModelRef},
    task::{Task, TaskResult},
    time::{Timestamp, now},
    tool::ToolOutcome,
};

fn agent() -> Agent {
    let at = now();
    Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("root"),
        name: "root".into(),
        scope: Scope::default(),
        model: ModelRef::new("mock", "m"),
        fallbacks: Vec::new(),
        status: AgentStatus::Created,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        archived: false,
        paused: false,
        paused_via: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        config: AgentConfig::default(),
        stats: AgentStats::default(),
        run: RunId::new(),
        created_at: at,
        updated_at: at,
    }
}

fn inbox_item(agent: AgentId, seq: u64) -> InboxItem {
    InboxItem {
        id: InboxItemId::new(),
        agent_id: agent,
        seq,
        kind: InboxKind::Wake {
            reason: "test".into(),
        },
        delivered_at: now(),
    }
}

fn submission(key: &str, at: Timestamp) -> aether_core::idempotency::RecordedSubmission {
    aether_core::idempotency::RecordedSubmission {
        key: IdempotencyKey(key.to_owned()),
        fingerprint: format!("fp-{key}"),
        scheme: aether_core::idempotency::FINGERPRINT_SCHEME,
        at,
        outcome: aether_core::idempotency::SubmissionOutcome::Message {
            message: aether_core::id::MessageId::new(),
        },
    }
}

fn recorded(
    agent: AgentId,
    invocation: InvocationId,
    call: ToolCallId,
    key: &str,
    content: &str,
) -> RecordedOutcome {
    RecordedOutcome {
        key: IdempotencyKey(key.to_owned()),
        agent_id: agent,
        invocation_id: invocation,
        call_id: call,
        tool: "bash".into(),
        at: now(),
        outcome: ToolOutcome::ok(content),
    }
}

fn artifact_named(name: &str) -> Artifact {
    Artifact {
        id: ArtifactId::new(),
        kind: aether_core::artifact::ArtifactKind::CommandOutput,
        name: name.into(),
        path: None,
        summary: String::new(),
        size_bytes: 3,
        content_type: None,
        digest: None,
        created_by: Actor::System,
        agent_id: None,
        task_id: None,
        created_at: now(),
        evicted_at: None,
    }
}

mod transactions;

mod plans;
mod queries;

mod memories;

mod memory_feedback;

mod submissions;

mod ledger;

mod artifacts;

mod commit_stalls;
mod disk_full;
mod expired_history;
mod history_retention;
mod outcomes;
mod repository_aliases;
mod retention_growth;

#[test]
fn final_sync_copies_committed_wal_while_the_store_is_still_held() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store =
        SqliteStore::open_checkpointing(&path, std::time::Duration::from_secs(3600)).unwrap();
    store
        .commit(Commit::new().event(EventDraft::new(
            Actor::System,
            EventKind::Custom {
                name: "final-sync".into(),
                payload: serde_json::json!({"done": true}),
            },
        )))
        .unwrap();
    assert!(store.sync().unwrap());
    let conn = Connection::open(&path).unwrap();
    let (frames, copied): (i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(NOOP)", [], |row| {
            Ok((row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert!(frames > 0);
    assert_eq!(
        frames, copied,
        "all final commits must be synced before runtime exit"
    );
}

#[test]
fn final_sync_reports_without_failing_when_a_reader_prevents_a_full_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store =
        SqliteStore::open_checkpointing(&path, std::time::Duration::from_secs(3600)).unwrap();
    assert!(store.sync().unwrap());
    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT * FROM events;")
        .unwrap();
    store
        .commit(Commit::new().event(EventDraft::new(
            Actor::System,
            EventKind::Custom {
                name: "after-reader".into(),
                payload: serde_json::Value::Null,
            },
        )))
        .unwrap();
    store
        .conn
        .lock()
        .busy_timeout(std::time::Duration::ZERO)
        .unwrap();
    assert!(
        !store.sync().unwrap(),
        "a held reader leaves frames in the WAL, not a store error"
    );
    reader.execute_batch("ROLLBACK").unwrap();
    assert!(store.sync().unwrap());
}
