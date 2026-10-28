//! SQLite schema and migrations.
//!
//! Every table stores the full record as JSON in `body` plus the columns
//! queries filter or sort on. The JSON is the source of truth for a row; the
//! columns are indexes.

use rusqlite::{Connection, OptionalExtension};

use crate::{Result, StoreError};

pub const SCHEMA_VERSION: u32 = 28;

const V1: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    seq             INTEGER PRIMARY KEY,
    id              TEXT NOT NULL UNIQUE,
    at              TEXT NOT NULL,
    type            TEXT NOT NULL,
    family          TEXT NOT NULL,
    agent_id        TEXT,
    task_id         TEXT,
    parent_agent_id TEXT,
    run             TEXT,
    body            TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS events_agent ON events(agent_id, seq);
CREATE INDEX IF NOT EXISTS events_parent ON events(parent_agent_id, seq);
CREATE INDEX IF NOT EXISTS events_task ON events(task_id, seq);
CREATE INDEX IF NOT EXISTS events_run ON events(run, seq);
CREATE INDEX IF NOT EXISTS events_type ON events(type, seq);

CREATE TABLE IF NOT EXISTS agents (
    id         TEXT PRIMARY KEY,
    parent     TEXT,
    role       TEXT NOT NULL,
    status     TEXT NOT NULL,
    run        TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    body       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS agents_parent ON agents(parent);
CREATE INDEX IF NOT EXISTS agents_status ON agents(status);
CREATE INDEX IF NOT EXISTS agents_run ON agents(run);

CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY,
    owner       TEXT,
    parent_task TEXT,
    status      TEXT NOT NULL,
    run         TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    body        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tasks_owner ON tasks(owner);
CREATE INDEX IF NOT EXISTS tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS tasks_parent ON tasks(parent_task);

CREATE TABLE IF NOT EXISTS inbox (
    id           TEXT PRIMARY KEY,
    agent_id     TEXT NOT NULL,
    seq          INTEGER NOT NULL,
    kind         TEXT NOT NULL,
    delivered_at TEXT NOT NULL,
    body         TEXT NOT NULL,
    UNIQUE(agent_id, seq)
);

CREATE TABLE IF NOT EXISTS leases (
    agent_id   TEXT PRIMARY KEY,
    holder     TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    body       TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS snapshots (
    id         TEXT PRIMARY KEY,
    agent_id   TEXT NOT NULL,
    turn       INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    body       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS snapshots_agent ON snapshots(agent_id, turn);

CREATE TABLE IF NOT EXISTS invocations (
    id          TEXT PRIMARY KEY,
    snapshot_id TEXT NOT NULL,
    agent_id    TEXT NOT NULL,
    turn        INTEGER NOT NULL,
    started_at  TEXT NOT NULL,
    body        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS invocations_agent ON invocations(agent_id, turn);

CREATE TABLE IF NOT EXISTS checkpoints (
    id         TEXT PRIMARY KEY,
    agent_id   TEXT NOT NULL,
    version    INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    body       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS checkpoints_agent ON checkpoints(agent_id, version);

CREATE TABLE IF NOT EXISTS approvals (
    id           TEXT PRIMARY KEY,
    agent_id     TEXT NOT NULL,
    call_id      TEXT NOT NULL,
    state        TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    body         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS approvals_state ON approvals(state, requested_at);

CREATE TABLE IF NOT EXISTS memories (
    id            TEXT PRIMARY KEY,
    scope         TEXT NOT NULL,
    kind          TEXT NOT NULL,
    text          TEXT NOT NULL,
    tags          TEXT NOT NULL,
    importance    REAL NOT NULL,
    superseded_by TEXT,
    created_at    TEXT NOT NULL,
    body          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS memories_scope ON memories(scope, superseded_by);

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    text, tags, content='memories', content_rowid='rowid'
);
CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, text, tags) VALUES (new.rowid, new.text, new.tags);
END;
CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, text, tags) VALUES ('delete', old.rowid, old.text, old.tags);
END;
CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, text, tags) VALUES ('delete', old.rowid, old.text, old.tags);
    INSERT INTO memories_fts(rowid, text, tags) VALUES (new.rowid, new.text, new.tags);
END;

CREATE TABLE IF NOT EXISTS artifacts (
    id         TEXT PRIMARY KEY,
    agent_id   TEXT,
    task_id    TEXT,
    created_at TEXT NOT NULL,
    body       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS artifacts_task ON artifacts(task_id);
"#;

const V2: &str = r#"
CREATE TABLE IF NOT EXISTS approval_rules (
    id         TEXT PRIMARY KEY,
    decision   TEXT NOT NULL,
    revoked    INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    body       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS approval_rules_active ON approval_rules(revoked, created_at);
"#;

const V3: &str = r#"
CREATE TABLE IF NOT EXISTS tool_outcomes (
    agent_id TEXT NOT NULL,
    key      TEXT NOT NULL,
    call_id  TEXT NOT NULL,
    tool     TEXT NOT NULL,
    at       TEXT NOT NULL,
    body     TEXT NOT NULL,
    PRIMARY KEY (agent_id, key)
);
"#;

const V4: &str = r#"
ALTER TABLE artifacts ADD COLUMN digest TEXT;
ALTER TABLE artifacts ADD COLUMN evicted INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS artifacts_digest ON artifacts(digest);
CREATE INDEX IF NOT EXISTS artifacts_live ON artifacts(evicted, created_at);
CREATE INDEX IF NOT EXISTS artifacts_agent ON artifacts(agent_id, created_at);
"#;

// A replay record is scoped to one invocation, so the V3 rows - keyed by
// agent alone - would memoize a live agent's calls forever. They are a
// replay cache, not history: dropping them is the fix.
const V5: &str = r#"
DROP TABLE IF EXISTS tool_outcomes;
CREATE TABLE tool_outcomes (
    agent_id      TEXT NOT NULL,
    invocation_id TEXT NOT NULL,
    key           TEXT NOT NULL,
    call_id       TEXT NOT NULL,
    tool          TEXT NOT NULL,
    at            TEXT NOT NULL,
    body          TEXT NOT NULL,
    PRIMARY KEY (agent_id, invocation_id, key)
);
"#;

// Two identical calls in one model response are two deliberate calls, so the
// call id belongs in the record's identity: without it the second replays
// the first's outcome when their approvals settle in separate rounds. Same
// reasoning as V5 - it is a replay cache, so it is dropped, not migrated.
const V6: &str = r#"
DROP TABLE IF EXISTS tool_outcomes;
CREATE TABLE tool_outcomes (
    agent_id      TEXT NOT NULL,
    invocation_id TEXT NOT NULL,
    call_id       TEXT NOT NULL,
    key           TEXT NOT NULL,
    tool          TEXT NOT NULL,
    at            TEXT NOT NULL,
    body          TEXT NOT NULL,
    PRIMARY KEY (agent_id, invocation_id, call_id, key)
);
"#;

// Curation: a person pins a memory in front of the model or forgets it. Both
// are projections of the record body, indexed because retrieval filters on
// them on every turn.
const V7: &str = r#"
ALTER TABLE memories ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN forgotten_at TEXT;
CREATE INDEX IF NOT EXISTS memories_live ON memories(scope, forgotten_at, superseded_by);
"#;

// Two things. The index answers "is this exact note already stored in this
// scope?", which is how seeding stays idempotent past one page of memories.
// The data migration re-derives repository scope ids: a store written before
// this carries whatever userinfo the `origin` remote had, and stripping it is
// not something SQL can express - see `normalize_repository_scopes`.
const V8: &str = r#"
CREATE INDEX IF NOT EXISTS memories_text ON memories(scope, text);
"#;

/// The statements that take the schema from version N-1 to N.
// Older runtimes must not ignore durable cancellation intent and dispatch
// descendants whose cancellation has not finished.
const V9: &str = r#"
INSERT OR IGNORE INTO meta(key, value) VALUES ('runtime.pending_cancellations', '{}');
"#;

// A recurring delivery and its next due time commit together. The unique
// occurrence key rejects accidental replay instead of replacing history.
const V10: &str = r#"
CREATE TABLE IF NOT EXISTS schedules (
    id TEXT PRIMARY KEY,
    target TEXT NOT NULL,
    next_due TEXT,
    body TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS schedules_due ON schedules(next_due) WHERE next_due IS NOT NULL;
CREATE INDEX IF NOT EXISTS schedules_target ON schedules(target, id);
CREATE TABLE IF NOT EXISTS schedule_occurrences (
    schedule TEXT NOT NULL REFERENCES schedules(id),
    due TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY(schedule, due)
);
"#;

// Client-supplied idempotency keys on the commands that create work.
// `key` is the primary key and the insert is a plain INSERT: a second
// record under one key is a duplicate creation the transaction must roll back,
// never a row to overwrite. Retention sweeps on the `(at, key)` index, which
// covers both passes: a range delete by age, and an oldest-first delete of the
// overflow that reads only as many entries as it removes.
const V11: &str = r#"
CREATE TABLE IF NOT EXISTS submissions (
    key         TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    at          TEXT NOT NULL,
    body        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS submissions_at ON submissions(at, key);
"#;

// Spawn schedules have no target, so `target` turns nullable: SQLite needs a
// table rebuild for that; the renames repoint the occurrences' foreign key.
const V12: &str = r#"
CREATE TABLE schedules_v12 (
    id TEXT PRIMARY KEY,
    target TEXT,
    next_due TEXT,
    body TEXT NOT NULL
);
INSERT INTO schedules_v12(id, target, next_due, body)
    SELECT id, target, next_due, body FROM schedules;
CREATE TABLE schedule_occurrences_v12 (
    schedule TEXT NOT NULL REFERENCES schedules_v12(id),
    due TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY(schedule, due)
);
INSERT INTO schedule_occurrences_v12(schedule, due, body)
    SELECT schedule, due, body FROM schedule_occurrences;
DROP TABLE schedule_occurrences;
DROP TABLE schedules;
ALTER TABLE schedules_v12 RENAME TO schedules;
ALTER TABLE schedule_occurrences_v12 RENAME TO schedule_occurrences;
CREATE INDEX schedules_due ON schedules(next_due) WHERE next_due IS NOT NULL;
CREATE INDEX schedules_target ON schedules(target, id);
"#;

// Payload-free spans let filtered replay distinguish expired history from an
// empty result. Empty identity columns represent events with no such identity.
const V13: &str = r#"
CREATE TABLE expired_event_spans (
    run TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    parent_agent_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    type TEXT NOT NULL,
    family TEXT NOT NULL,
    first_seq INTEGER NOT NULL CHECK(first_seq > 0),
    last_seq INTEGER NOT NULL CHECK(last_seq >= first_seq),
    PRIMARY KEY(run, agent_id, parent_agent_id, task_id, type)
);
CREATE INDEX expired_events_agent ON expired_event_spans(agent_id, last_seq);
CREATE INDEX expired_events_parent ON expired_event_spans(parent_agent_id, last_seq);
CREATE INDEX expired_events_task ON expired_event_spans(task_id, last_seq);
CREATE INDEX expired_events_sequence ON expired_event_spans(last_seq);
"#;

// An agent's latest event of one type (the scheduler asks several per tick)
// otherwise walks `events_type` through every agent's events of that type.
const V14: &str = r#"
CREATE INDEX events_agent_type ON events(agent_id, type, seq);
"#;

const V15: &str = r#"
CREATE INDEX artifacts_live_path ON artifacts(json_extract(body, '$.path')) WHERE evicted = 0;
ALTER TABLE artifacts ADD COLUMN unlink_pending INTEGER NOT NULL DEFAULT 0;
UPDATE artifacts SET unlink_pending = 1 WHERE evicted = 1;
CREATE INDEX artifacts_unlink_pending ON artifacts(id) WHERE unlink_pending = 1;
CREATE TRIGGER artifacts_insert_evicted AFTER INSERT ON artifacts WHEN NEW.evicted = 1
BEGIN
    UPDATE artifacts SET unlink_pending = 1 WHERE id = NEW.id;
END;
CREATE TRIGGER artifacts_mark_evicted AFTER UPDATE OF evicted ON artifacts
WHEN NEW.evicted = 1 AND OLD.evicted = 0
BEGIN
    UPDATE artifacts SET unlink_pending = 1 WHERE id = NEW.id;
END;
"#;

// Web Push subscriptions, one per paired device. The body is opaque here.
const V16: &str = r#"
CREATE TABLE push_subscriptions (
    device     TEXT PRIMARY KEY,
    updated_at TEXT NOT NULL,
    body       TEXT NOT NULL
);
"#;

// Paired remote devices. `last_seen_at` is the one field outside `body`:
// every device hello stamps it, and must not race a rename of the record.
const V17: &str = r#"
CREATE TABLE devices (
    id           TEXT PRIMARY KEY,
    created_at   TEXT NOT NULL,
    revoked_at   TEXT,
    last_seen_at TEXT,
    body         TEXT NOT NULL
);
CREATE INDEX devices_created ON devices(created_at, id);
"#;

// Receipts are numbered as recorded (`SubmissionLedger`). Rows from before
// the ledger keep 0: no horizon was ever issued below them.
const V18: &str = r#"
ALTER TABLE submissions ADD COLUMN ordinal INTEGER NOT NULL DEFAULT 0;
"#;

// No table changes. Record bodies may now hold `image_ref` content blocks,
// which a binary before this cannot read: the version alone keeps it out.
const V19: &str = "";

// An old repository name and the id its memory now lives under. One hop by
// construction: the store refuses an alias of an alias, or of one's canonical.
const V20: &str = r#"
CREATE TABLE repository_aliases (
    alias      TEXT PRIMARY KEY,
    canonical  TEXT NOT NULL CHECK(canonical <> alias),
    created_at TEXT NOT NULL,
    body       TEXT NOT NULL
);
CREATE INDEX repository_aliases_canonical ON repository_aliases(canonical);
"#;

const V21: &str = r#"
ALTER TABLE memories ADD COLUMN proposed INTEGER NOT NULL DEFAULT 0 CHECK (proposed IN (0, 1));
CREATE INDEX memories_proposals ON memories(scope, proposed, superseded_by, forgotten_at);
"#;

const V22: &str = r#"
CREATE TABLE memory_feedback (
    id           TEXT PRIMARY KEY,
    memory_id    TEXT NOT NULL,
    task_id      TEXT NOT NULL,
    verdict      TEXT NOT NULL CHECK (verdict IN ('helped','hurt','neutral')),
    evidence     TEXT NOT NULL CHECK (evidence IN ('operator','review','outcome')),
    created_at   TEXT NOT NULL,
    retracted_at TEXT,
    body         TEXT NOT NULL
);
CREATE INDEX memory_feedback_memory_created ON memory_feedback(memory_id, created_at);
CREATE INDEX memory_feedback_ranked ON memory_feedback(memory_id, task_id, created_at DESC, id DESC, verdict)
    WHERE retracted_at IS NULL AND evidence IN ('operator','review');
"#;

// Agent and task lists sort by creation: these indexes stream them in order,
// whole or per run, instead of sorting bodies (a sort past ~2 MB spills to disk).
const V23: &str = r#"
CREATE INDEX agents_created ON agents(created_at);
CREATE INDEX tasks_created ON tasks(created_at);
DROP INDEX IF EXISTS agents_run;
CREATE INDEX agents_run_created ON agents(run, created_at);
CREATE INDEX tasks_run_created ON tasks(run, created_at);
"#;

// No foreign key: ownership must outlive the agent and run history.
const V24: &str = r#"
CREATE TABLE engine_session_ownership (
    engine     TEXT NOT NULL,
    session_id TEXT NOT NULL,
    body       TEXT NOT NULL,
    PRIMARY KEY (engine, session_id)
);
"#;

// Lease renewals and memory admission events were committed without a run,
// which pinned the run against history retention. Give those agent events the
// run their agent belongs to, in the column and in the stored body.
const V25: &str = r#"
UPDATE events
SET run = (SELECT agents.run FROM agents WHERE agents.id = events.agent_id),
    body = json_set(events.body, '$.run',
                    (SELECT agents.run FROM agents WHERE agents.id = events.agent_id))
WHERE run IS NULL
  AND type IN ('lease.renewed', 'memory.candidate', 'memory.accepted',
               'memory.rejected', 'memory.superseded')
  AND agent_id IN (SELECT id FROM agents);
"#;

// The notification center. A poster's key names one notification per source,
// so a repost updates it. Idempotent, so a store whose stamp was rewound
// replays it.
const V26: &str = r#"
CREATE TABLE IF NOT EXISTS notifications (
    id         TEXT PRIMARY KEY,
    source     TEXT NOT NULL,
    key        TEXT,
    state      TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    body       TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS notifications_key ON notifications(source, key) WHERE key IS NOT NULL;
CREATE INDEX IF NOT EXISTS notifications_state ON notifications(state);
"#;

// Large strings inside event bodies (tool output, long messages) move out of
// the hot events table into compressed, content-addressed blobs: an output
// recorded twice is stored once. Deleting an event releases its references,
// and a blob goes with its last one, so history expiry needs no extra step.
const V27: &str = r#"
CREATE TABLE IF NOT EXISTS event_blobs (
    digest TEXT PRIMARY KEY,
    size   INTEGER NOT NULL,
    data   BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS event_blob_refs (
    seq    INTEGER NOT NULL,
    digest TEXT NOT NULL,
    PRIMARY KEY (seq, digest)
);
CREATE INDEX IF NOT EXISTS event_blob_refs_digest ON event_blob_refs(digest);
CREATE TRIGGER IF NOT EXISTS events_release_blobs AFTER DELETE ON events BEGIN
    DELETE FROM event_blob_refs WHERE seq = old.seq;
END;
CREATE TRIGGER IF NOT EXISTS event_blob_refs_collect AFTER DELETE ON event_blob_refs BEGIN
    DELETE FROM event_blobs WHERE digest = old.digest
        AND NOT EXISTS (SELECT 1 FROM event_blob_refs WHERE digest = old.digest);
END;
"#;

// History retention asks whether a run's agents hold a pending approval, and
// without this index every such question read the whole approvals table.
const V28: &str = r#"
CREATE INDEX IF NOT EXISTS approvals_agent ON approvals(agent_id);
"#;

const MIGRATIONS: [&str; SCHEMA_VERSION as usize] = [
    V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11, V12, V13, V14, V15, V16, V17, V18, V19, V20, V21,
    V22, V23, V24, V25, V26, V27, V28,
];

/// Rewrites that SQL cannot express, run in the same transaction as the
/// schema block for the version they belong to.
fn data_migration(tx: &Connection, to: u32) -> Result<()> {
    match to {
        8 => normalize_repository_scopes(tx),
        _ => Ok(()),
    }
}

/// Re-derive every stored repository scope id through
/// [`aether_core::repository::normalize_id`], so a credential that reached
/// the store as part of an `origin` URL does not survive the upgrade.
///
/// Rows whose id is already normal are left untouched. The event log is
/// append-only and is deliberately not rewritten: history is not editable,
/// which is why the derivation itself had to be fixed.
///
/// A row whose body will not parse is skipped and warned about, memories and
/// agents alike: it is already unreadable to everything that loads it, and
/// failing the whole migration on one leaves no way past it.
fn normalize_repository_scopes(tx: &Connection) -> Result<()> {
    let mut memories: Vec<(String, String)> = Vec::new();
    {
        let mut stmt = tx.prepare("SELECT id, body FROM memories WHERE scope LIKE 'repo:%'")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            memories.push(row?);
        }
    }
    for (id, body) in memories {
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&body) else {
            tracing::warn!(memory = %id, "memory body is not JSON; left as it is");
            continue;
        };
        let Some(raw) = value.pointer("/scope/id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(normal) = aether_core::repository::normalize_id(raw) else {
            continue;
        };
        if normal == raw {
            continue;
        }
        value["scope"]["id"] = serde_json::Value::String(normal.clone());
        tx.execute(
            "UPDATE memories SET scope = ?1, body = ?2 WHERE id = ?3",
            rusqlite::params![format!("repo:{normal}"), value.to_string(), id],
        )?;
    }

    let mut agents: Vec<(String, String)> = Vec::new();
    {
        let mut stmt = tx.prepare("SELECT id, body FROM agents")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            agents.push(row?);
        }
    }
    for (id, body) in agents {
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&body) else {
            tracing::warn!(agent = %id, "agent body is not JSON; left as it is");
            continue;
        };
        let Some(raw) = value.pointer("/scope/repository").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(normal) = aether_core::repository::normalize_id(raw) else {
            continue;
        };
        if normal == raw {
            continue;
        }
        value["scope"]["repository"] = serde_json::Value::String(normal);
        tx.execute(
            "UPDATE agents SET body = ?1 WHERE id = ?2",
            rusqlite::params![value.to_string(), id],
        )?;
    }
    Ok(())
}

pub(crate) fn current_version(conn: &Connection) -> Result<Option<u32>> {
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta')",
        [],
        |row| row.get(0),
    )?;
    if !has_meta {
        return Ok(None);
    }
    // A failed read is not a fresh store: replaying V1 onwards over real
    // tables fails on the first non-idempotent block, naming the wrong problem.
    let stamp: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    let found = match stamp {
        Some(raw) => Some(raw.parse().map_err(|_| {
            StoreError::Corrupt(format!("schema_version is not a number: `{raw}`"))
        })?),
        None => None,
    };
    Ok(found)
}

/// A binary cannot trust itself to read records a newer one wrote.
fn refuse_newer_store(found: Option<u32>, supported: u32) -> Result<()> {
    match found {
        Some(found) if found > supported => Err(StoreError::SchemaTooNew { found, supported }),
        _ => Ok(()),
    }
}

/// Create or upgrade the schema. Refuses a store written by a newer version.
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    let found = current_version(conn)?;
    refuse_newer_store(found, SCHEMA_VERSION)?;
    if found == Some(SCHEMA_VERSION) {
        return Ok(());
    }
    // One transaction: the statements and the version stamp commit together,
    // so an interrupted upgrade leaves the store at the version it was, ready
    // to try the same blocks again. Not every block is idempotent (V4 uses
    // ALTER), which is exactly why this must be atomic.
    let tx = conn.unchecked_transaction()?;
    for (i, statements) in MIGRATIONS
        .iter()
        .enumerate()
        .skip(found.unwrap_or(0) as usize)
    {
        tx.execute_batch(statements)?;
        data_migration(&tx, i as u32 + 1)?;
    }
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests;
