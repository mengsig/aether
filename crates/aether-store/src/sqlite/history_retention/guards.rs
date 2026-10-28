//! The durable references that keep a finished run (`?1`) from expiring.
//! `{rows}` limits each guard's own table (`src`) to some rows: every row for
//! a full check, or only the rows written since a screen (`?2`).

pub(super) struct Guard {
    pub reason: &'static str,
    pub table: &'static str,
    pub sql: &'static str,
}

// Protect references before removing their target rows. Even resolved external
// references are retained here; batched dependency-closed expiry can relax this
// later without making one-run deletion leave dangling state.
pub(super) const GUARDS: &[Guard] = &[
    Guard {
        reason: "memory scope or provenance",
        table: "memories",
        sql: "SELECT EXISTS(SELECT 1 FROM memories AS src WHERE {rows} AND (json_extract(body,'$.scope.id') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.scope.root') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.provenance.agent') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.provenance.task') IN (SELECT id FROM tasks WHERE run=?1) OR json_extract(body,'$.provenance.event') IN (SELECT id FROM events WHERE run=?1)))",
    },
    Guard {
        reason: "approval rule",
        table: "approval_rules",
        sql: "SELECT EXISTS(SELECT 1 FROM approval_rules AS src WHERE {rows} AND (json_extract(body,'$.scope.id')=?1 OR json_extract(body,'$.scope.id') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.scope.root') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.created_by.id') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.revoked.by.id') IN (SELECT id FROM agents WHERE run=?1)))",
    },
    Guard {
        reason: "lease",
        table: "leases",
        sql: "SELECT EXISTS(SELECT 1 FROM leases AS src WHERE {rows} AND agent_id IN (SELECT id FROM agents WHERE run=?1))",
    },
    Guard {
        reason: "pending approval",
        table: "approvals",
        sql: "SELECT EXISTS(SELECT 1 FROM approvals AS src WHERE {rows} AND state='pending' AND agent_id IN (SELECT id FROM agents WHERE run=?1))",
    },
    Guard {
        reason: "schedule",
        table: "schedules",
        sql: "SELECT EXISTS(SELECT 1 FROM schedules AS src WHERE {rows} AND (target IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.run')=?1 OR json_extract(body,'$.owner.id') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.last_spawned') IN (SELECT id FROM agents WHERE run=?1)))",
    },
    Guard {
        reason: "schedule occurrence",
        table: "schedule_occurrences",
        sql: "SELECT EXISTS(SELECT 1 FROM schedule_occurrences AS src WHERE {rows} AND json_extract(body,'$.outcome.run')=?1)",
    },
    Guard {
        reason: "artifact",
        table: "artifacts",
        sql: "SELECT EXISTS(SELECT 1 FROM artifacts AS src WHERE {rows} AND (agent_id IN (SELECT id FROM agents WHERE run=?1) OR task_id IN (SELECT id FROM tasks WHERE run=?1) OR json_extract(body,'$.created_by.id') IN (SELECT id FROM agents WHERE run=?1)))",
    },
    Guard {
        reason: "external snapshot task reference",
        table: "snapshots",
        sql: "SELECT EXISTS(SELECT 1 FROM snapshots AS src WHERE {rows} AND agent_id NOT IN (SELECT id FROM agents WHERE run=?1) AND json_extract(body,'$.task_id') IN (SELECT id FROM tasks WHERE run=?1))",
    },
    Guard {
        reason: "external invocation snapshot reference",
        table: "invocations",
        sql: "SELECT EXISTS(SELECT 1 FROM invocations AS src WHERE {rows} AND agent_id NOT IN (SELECT id FROM agents WHERE run=?1) AND snapshot_id IN (SELECT id FROM snapshots WHERE agent_id IN (SELECT id FROM agents WHERE run=?1)))",
    },
    Guard {
        reason: "external checkpoint provenance",
        table: "checkpoints",
        sql: "SELECT EXISTS(SELECT 1 FROM checkpoints AS src WHERE {rows} AND agent_id NOT IN (SELECT id FROM agents WHERE run=?1) AND (json_extract(body,'$.provenance.source_snapshot') IN (SELECT id FROM snapshots WHERE agent_id IN (SELECT id FROM agents WHERE run=?1)) OR json_extract(body,'$.provenance.previous') IN (SELECT id FROM checkpoints WHERE agent_id IN (SELECT id FROM agents WHERE run=?1))))",
    },
    Guard {
        reason: "external agent checkpoint reference",
        table: "agents",
        sql: "SELECT EXISTS(SELECT 1 FROM agents AS src WHERE {rows} AND run<>?1 AND json_extract(body,'$.checkpoint') IN (SELECT id FROM checkpoints WHERE agent_id IN (SELECT id FROM agents WHERE run=?1)))",
    },
    Guard {
        reason: "external or unscoped event reference",
        table: "events",
        sql: "SELECT EXISTS(SELECT 1 FROM events AS src WHERE {rows} AND (run IS NULL OR run<>?1) AND (agent_id IN (SELECT id FROM agents WHERE run=?1) OR parent_agent_id IN (SELECT id FROM agents WHERE run=?1) OR task_id IN (SELECT id FROM tasks WHERE run=?1)))",
    },
    Guard {
        reason: "external agent reference",
        table: "agents",
        sql: "SELECT EXISTS(SELECT 1 FROM agents AS src, json_tree(src.body) WHERE {rows} AND src.run<>?1 AND json_tree.type='text' AND (json_tree.value IN (SELECT id FROM agents WHERE run=?1) OR json_tree.value IN (SELECT id FROM tasks WHERE run=?1)))",
    },
    Guard {
        reason: "external task reference",
        table: "tasks",
        sql: "SELECT EXISTS(SELECT 1 FROM tasks AS src WHERE {rows} AND run<>?1 AND (owner IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.creator.id') IN (SELECT id FROM agents WHERE run=?1) OR json_extract(body,'$.parent_task') IN (SELECT id FROM tasks WHERE run=?1) OR EXISTS(SELECT 1 FROM json_each(src.body,'$.dependencies') WHERE value IN (SELECT id FROM tasks WHERE run=?1))))",
    },
    Guard {
        reason: "external inbox reference",
        table: "inbox",
        sql: "SELECT EXISTS(SELECT 1 FROM inbox AS src, json_tree(src.body) WHERE {rows} AND src.agent_id NOT IN (SELECT id FROM agents WHERE run=?1) AND json_tree.type='text' AND (json_tree.value IN (SELECT id FROM agents WHERE run=?1) OR json_tree.value IN (SELECT id FROM tasks WHERE run=?1) OR json_tree.value IN (SELECT id FROM approvals WHERE agent_id IN (SELECT id FROM agents WHERE run=?1))))",
    },
    Guard {
        reason: "external approval reference",
        table: "approvals",
        sql: "SELECT EXISTS(SELECT 1 FROM approvals AS src WHERE {rows} AND agent_id NOT IN (SELECT id FROM agents WHERE run=?1) AND json_extract(body,'$.task_id') IN (SELECT id FROM tasks WHERE run=?1))",
    },
    Guard {
        reason: "submission replay record",
        table: "submissions",
        sql: "SELECT EXISTS(SELECT 1 FROM submissions AS src, json_tree(src.body) WHERE {rows} AND json_tree.type='text' AND (json_tree.value IN (SELECT id FROM agents WHERE run=?1) OR json_tree.value IN (SELECT id FROM tasks WHERE run=?1)))",
    },
    Guard {
        reason: "pending cancellation",
        table: "meta",
        sql: "SELECT EXISTS(SELECT 1 FROM meta AS src, json_each(src.value) WHERE {rows} AND src.key='runtime.pending_cancellations' AND json_each.key IN (SELECT id FROM agents WHERE run=?1))",
    },
];

/// The guard over its whole table.
pub(super) fn whole(guard: &Guard) -> String {
    guard.sql.replace("{rows}", "1")
}

/// The guard over the rows of its table listed in `?2`, a JSON array of rowids.
pub(super) fn written(guard: &Guard) -> String {
    guard
        .sql
        .replace("{rows}", "src.rowid IN (SELECT value FROM json_each(?2))")
}

/// Whether a write since the screen gave `run` a member these guards read:
/// a guard then judges sets the screen never saw, so expiry checks in full.
pub(super) const RUN_MEMBERS: &[(&str, &str)] = &[
    ("agents", "run=?1"),
    ("tasks", "run=?1"),
    ("events", "run=?1"),
    (
        "snapshots",
        "agent_id IN (SELECT id FROM agents WHERE run=?1)",
    ),
    (
        "checkpoints",
        "agent_id IN (SELECT id FROM agents WHERE run=?1)",
    ),
    (
        "approvals",
        "agent_id IN (SELECT id FROM agents WHERE run=?1)",
    ),
];
