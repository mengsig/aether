use super::write_tally::WriteTally;
use super::{SqliteStore, decision_key, schedule_time, scope_key, submission_time};
use crate::{Mutation, Result, StoreError};
use aether_core::tool::ApprovalStatus;
use rusqlite::{Transaction, params};

impl SqliteStore {
    pub(super) fn apply(
        tx: &Transaction<'_>,
        mutation: Mutation,
        tally: &mut WriteTally,
    ) -> Result<()> {
        match mutation {
            Mutation::RecordEngineSessionOwnership(record) => {
                super::engine_sessions::record(tx, record, tally)?;
            }
            Mutation::UpsertAgent(agent) => {
                tx.execute(
                    "INSERT INTO agents(id, parent, role, status, run, created_at, updated_at, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(id) DO UPDATE SET parent = excluded.parent, role = excluded.role,
                       status = excluded.status, updated_at = excluded.updated_at, body = excluded.body",
                    params![
                        agent.id.to_string(),
                        agent.parent.map(|p| p.to_string()),
                        agent.role.as_str(),
                        agent.status.kind().as_str(),
                        agent.run.to_string(),
                        agent.created_at.to_rfc3339(),
                        agent.updated_at.to_rfc3339(),
                        tally.json("agents", &agent)?,
                    ],
                )?;
            }
            Mutation::UpsertTask(task) => {
                tx.execute(
                    "INSERT INTO tasks(id, owner, parent_task, status, run, created_at, updated_at, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(id) DO UPDATE SET owner = excluded.owner, parent_task = excluded.parent_task,
                       status = excluded.status, updated_at = excluded.updated_at, body = excluded.body",
                    params![
                        task.id.to_string(),
                        task.owner.map(|o| o.to_string()),
                        task.parent_task.map(|p| p.to_string()),
                        task.status.kind().as_str(),
                        task.run.to_string(),
                        task.created_at.to_rfc3339(),
                        task.updated_at.to_rfc3339(),
                        tally.json("tasks", &task)?,
                    ],
                )?;
            }
            Mutation::DeliverInbox(item) => {
                let last: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(seq), 0) FROM inbox WHERE agent_id = ?1",
                    [item.agent_id.to_string()],
                    |r| r.get(0),
                )?;
                let last = last as u64;
                if item.seq != last + 1 {
                    return Err(StoreError::InboxSequence {
                        agent: item.agent_id,
                        expected: last + 1,
                        got: item.seq,
                    });
                }
                let kind = serde_json::to_value(&item.kind)?;
                let kind = kind
                    .get("kind")
                    .and_then(|k| k.as_str())
                    .unwrap_or("unknown")
                    .to_owned();
                tx.execute(
                    "INSERT INTO inbox(id, agent_id, seq, kind, delivered_at, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        item.id.to_string(),
                        item.agent_id.to_string(),
                        item.seq as i64,
                        kind,
                        item.delivered_at.to_rfc3339(),
                        tally.json("inbox", &item)?,
                    ],
                )?;
            }
            Mutation::SetLease(lease) => {
                tx.execute(
                    "INSERT INTO leases(agent_id, holder, expires_at, body) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(agent_id) DO UPDATE SET holder = excluded.holder,
                       expires_at = excluded.expires_at, body = excluded.body",
                    params![
                        lease.agent_id.to_string(),
                        lease.holder,
                        lease.expires_at.to_rfc3339(),
                        tally.json("leases", &lease)?,
                    ],
                )?;
            }
            Mutation::ClearLease(agent) => {
                tx.execute(
                    "DELETE FROM leases WHERE agent_id = ?1",
                    [agent.to_string()],
                )?;
            }
            Mutation::PutSnapshot(snapshot) => {
                tx.execute(
                    "INSERT OR REPLACE INTO snapshots(id, agent_id, turn, created_at, body) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        snapshot.id.to_string(),
                        snapshot.agent_id.to_string(),
                        snapshot.turn as i64,
                        snapshot.created_at.to_rfc3339(),
                        tally.json("snapshots", &snapshot)?,
                    ],
                )?;
            }
            Mutation::PutInvocation(inv) => {
                tx.execute(
                    "INSERT OR REPLACE INTO invocations(id, snapshot_id, agent_id, turn, started_at, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        inv.id.to_string(),
                        inv.snapshot_id.to_string(),
                        inv.agent_id.to_string(),
                        inv.turn as i64,
                        inv.started_at.to_rfc3339(),
                        tally.json("invocations", &inv)?,
                    ],
                )?;
            }
            Mutation::PutCheckpoint(cp) => {
                tx.execute(
                    "INSERT OR REPLACE INTO checkpoints(id, agent_id, version, created_at, body) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        cp.id.to_string(),
                        cp.agent_id.to_string(),
                        cp.version as i64,
                        cp.created_at.to_rfc3339(),
                        tally.json("checkpoints", &cp)?,
                    ],
                )?;
            }
            Mutation::PutApproval(ap) => {
                let state = match ap.status {
                    ApprovalStatus::Pending => "pending",
                    ApprovalStatus::Resolved { .. } => "resolved",
                };
                tx.execute(
                    "INSERT INTO approvals(id, agent_id, call_id, state, requested_at, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET state = excluded.state, body = excluded.body",
                    params![
                        ap.id.to_string(),
                        ap.agent_id.to_string(),
                        ap.call_id.to_string(),
                        state,
                        ap.requested_at.to_rfc3339(),
                        tally.json("approvals", &ap)?,
                    ],
                )?;
            }
            Mutation::PutApprovalRule(rule) => {
                tx.execute(
                    "INSERT INTO approval_rules(id, decision, revoked, created_at, body) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(id) DO UPDATE SET revoked = excluded.revoked, body = excluded.body",
                    params![
                        rule.id.to_string(),
                        decision_key(rule.decision),
                        i64::from(rule.revoked.is_some()),
                        rule.created_at.to_rfc3339(),
                        tally.json("approval_rules", &rule)?,
                    ],
                )?;
            }
            Mutation::RecordToolOutcome(rec) => {
                // A repeat of a recorded outcome writes nothing, so counts nothing.
                let body = serde_json::to_string(&rec)?;
                let inserted = tx.execute(
                    "INSERT INTO tool_outcomes(agent_id, invocation_id, call_id, key, tool, at, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(agent_id, invocation_id, call_id, key) DO NOTHING",
                    params![
                        rec.agent_id.to_string(),
                        rec.invocation_id.to_string(),
                        rec.call_id.to_string(),
                        rec.key.0.clone(),
                        rec.tool.clone(),
                        rec.at.to_rfc3339(),
                        body,
                    ],
                )?;
                debug_assert!(inserted <= 1, "one outcome per key");
                if inserted == 1 {
                    tally.row("tool_outcomes", body.len());
                }
            }
            Mutation::RecordSubmission(rec) => {
                // Numbered in this transaction, so a refused insert takes its
                // ordinal back and the ledger never counts an unrecorded one.
                let ordinal = super::ledger::ordinal_to_sql(Self::next_submission_ordinal(tx)?)?;
                // Plain INSERT: a key already recorded means this commit is a
                // duplicate creation, and failing the transaction is what
                // stops it becoming durable. The runtime checks the key under
                // the commit lock first, so reaching this is a bug, not a race.
                tx.execute(
                    "INSERT INTO submissions(key, fingerprint, at, body, ordinal)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        rec.key.0.clone(),
                        rec.fingerprint.clone(),
                        submission_time(rec.at),
                        tally.json("submissions", &rec)?,
                        ordinal,
                    ],
                )?;
            }
            Mutation::RecordMemoryFeedback(record) => {
                Self::insert_memory_feedback(tx, &record, tally)?;
            }
            Mutation::RetractMemoryFeedback { id, at, reason } => {
                Self::retract_memory_feedback(tx, id, at, &reason)?;
                tally.row("memory_feedback", reason.len());
            }
            Mutation::PutMemory(mem) => put_memory(tx, &mem, tally)?,
            Mutation::PutArtifact(art) => {
                tx.execute(
                    "INSERT OR REPLACE INTO artifacts(id, agent_id, task_id, created_at, digest, evicted, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        art.id.to_string(),
                        art.agent_id.map(|a| a.to_string()),
                        art.task_id.map(|t| t.to_string()),
                        art.created_at.to_rfc3339(),
                        art.digest.clone(),
                        i64::from(art.evicted_at.is_some()),
                        tally.json("artifacts", &art)?,
                    ],
                )?;
            }
            Mutation::UpsertSchedule(job) => {
                tx.execute(
                    "INSERT INTO schedules(id, target, next_due, body) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(id) DO UPDATE SET target=excluded.target, next_due=excluded.next_due, body=excluded.body",
                    params![job.id.to_string(), job.target().map(|id| id.to_string()), job.status.next_due().map(schedule_time), tally.json("schedules", &job)?],
                )?;
            }
            Mutation::RecordScheduleOccurrence(occurrence) => {
                tx.execute(
                    "INSERT INTO schedule_occurrences(schedule, due, body) VALUES (?1, ?2, ?3)",
                    params![
                        occurrence.schedule.to_string(),
                        schedule_time(occurrence.due),
                        tally.json("schedule_occurrences", &occurrence)?
                    ],
                )?;
            }
            Mutation::SetMeta { key, value } => {
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )?;
                tally.row("meta", key.len() + value.len());
            }
            Mutation::PutDevice(device) => Self::put_device(tx, &device, tally)?,
            Mutation::TouchDevice { id, at } => Self::touch_device(tx, id, at, tally)?,
            Mutation::PutNotification(notification) => {
                super::notifications::put(tx, &notification, tally)?;
            }
            Mutation::PruneResolvedNotifications { keep } => {
                super::notifications::prune_resolved(tx, keep)?;
            }
        }
        Ok(())
    }
}

fn put_memory(
    tx: &Transaction<'_>,
    mem: &aether_core::memory::MemoryRecord,
    tally: &mut WriteTally,
) -> Result<()> {
    if (mem.accepted_at.is_some() && mem.proposed_at.is_none())
        || (mem.is_proposed() && mem.superseded_by.is_some())
    {
        return Err(StoreError::Corrupt(format!(
            "memory {} has inconsistent proposal state; run `aether doctor`",
            mem.id
        )));
    }
    let kind = serde_json::to_value(mem.kind)?;
    let kind = kind
        .as_str()
        .ok_or_else(|| StoreError::Corrupt("MemoryKind is not a string".into()))?;
    let written = tx.execute(
        "INSERT INTO memories(id, scope, kind, text, tags, importance, superseded_by, created_at, body, pinned, forgotten_at, proposed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(id) DO UPDATE SET text = excluded.text, tags = excluded.tags,
           importance = excluded.importance, superseded_by = excluded.superseded_by, body = excluded.body,
           pinned = excluded.pinned, forgotten_at = excluded.forgotten_at, proposed = excluded.proposed",
        params![
            mem.id.to_string(), scope_key(&mem.scope), kind, mem.text, mem.tags.join(" "),
            mem.importance as f64, mem.superseded_by.map(|m| m.to_string()),
            mem.created_at.to_rfc3339(), tally.json("memories", mem)?, mem.pinned as i64,
            mem.forgotten_at.map(|t| t.to_rfc3339()), mem.is_proposed(),
        ],
    )?;
    debug_assert_eq!(written, 1);
    Ok(())
}
