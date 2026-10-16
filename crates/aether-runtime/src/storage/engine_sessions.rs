use std::collections::{BTreeMap, HashMap, HashSet};

use aether_core::agent::{Agent, AgentStatusKind};
use aether_store::{AgentFilter, EngineSessionOwnership};

use super::{FileDecision, FileReport, MAX_ENTRIES, Runtime, SystemTime, invalid};

mod file;
use file::LogFile;

#[cfg(test)]
mod tests;

struct SessionScan {
    files: Vec<(std::path::PathBuf, LogFile)>,
    truncated: bool,
}

impl Runtime {
    pub(super) fn gc_engine_sessions(
        &self,
        days: u16,
        now: SystemTime,
        dry_run: bool,
    ) -> crate::Result<serde_json::Value> {
        let scan = self.inspect_engine_sessions()?;
        self.finish_engine_session_gc(scan, days, now, dry_run)
    }

    fn inspect_engine_sessions(&self) -> crate::Result<SessionScan> {
        let ownership = self.store.engine_session_ownerships()?;
        let paths = owned_paths(&ownership);
        let previous = self.storage_last_run.lock().clone();
        let cursor = previous
            .as_ref()
            .and_then(|value| value["engine_sessions"]["scan_after"].as_str())
            .map(std::path::PathBuf::from);
        let ordered: Vec<_> = paths.keys().copied().collect();
        let start = cursor
            .as_ref()
            .and_then(|cursor| ordered.iter().position(|path| *path > cursor))
            .unwrap_or(0);
        // Inspect outside the gate, then recheck durable admission and file identity
        // under it. No native directory scan or filename discovery grants ownership.
        let candidates = ordered[start..]
            .iter()
            .chain(&ordered[..start])
            .take(MAX_ENTRIES)
            .map(|path| {
                LogFile::inspect(path, &paths[path])
                    .map(|file| ((*path).clone(), file))
                    .map_err(|error| {
                        invalid(format!("inspect engine log {}: {error}", path.display()))
                    })
            })
            .collect::<crate::Result<Vec<_>>>()?;
        Ok(SessionScan {
            files: candidates,
            truncated: paths.len() > MAX_ENTRIES,
        })
    }

    fn finish_engine_session_gc(
        &self,
        scan: SessionScan,
        days: u16,
        now: SystemTime,
        dry_run: bool,
    ) -> crate::Result<serde_json::Value> {
        let scan_after = scan.files.last().map(|(path, _)| path.clone());
        let mut report = FileReport {
            truncated: scan.truncated,
            ..Default::default()
        };
        let _hold = (!dry_run).then(|| self.hold());
        let ownership = self.store.engine_session_ownerships()?;
        let current_paths = owned_paths(&ownership);
        let agents = self.store.agents(&AgentFilter::default())?;
        let agents_by_id: HashMap<_, _> = agents.iter().map(|agent| (agent.id, agent)).collect();
        let mut references: HashMap<&str, Vec<_>> = HashMap::new();
        for agent in &agents {
            if let Some(session) = agent.engine_session.as_deref() {
                references.entry(session).or_default().push(agent.id);
            }
        }
        let sched = self.sched.lock();
        let engine_turns = self.engine_turns.lock();
        let in_flight = self.in_flight_agents(&sched, &engine_turns)?;
        // Preview is a point-in-time snapshot. Only deletion reserves admission
        // while rechecking and unlinking files.
        let _admission = (!dry_run).then_some((sched, engine_turns));
        let mut identities = HashMap::new();
        for (_, file) in &scan.files {
            if let Some(identity) = file.identity() {
                *identities.entry(identity).or_insert(0) += 1;
            }
        }
        let mut unavailable_paths = 0;
        for (path, file) in scan.files {
            let bytes = file.bytes();
            report.bytes += bytes;
            unavailable_paths += usize::from(file.unavailable());
            let records = current_paths.get(&path);
            let reason = if days == 0 {
                Some("engine session cleanup disabled")
            } else if let Some(reason) = file.blocker() {
                Some(reason)
            } else if file.identity().is_some_and(|id| identities[&id] > 1) {
                Some("ambiguous file identity")
            } else {
                match records.map(Vec::as_slice) {
                    Some([record]) => {
                        let owner = agents_by_id.get(&record.agent_id).copied();
                        admission(record, &path, owner, &references, &in_flight, days, now)
                            .or_else(|| file.retention_blocker(record, days, now))
                    }
                    _ => Some("ambiguous or changed ownership"),
                }
            };
            let (action, reason) = if let Some(reason) = reason {
                ("kept", reason)
            } else if let Some(reason) = file.recheck().map_err(|error| {
                invalid(format!("recheck engine log {}: {error}", path.display()))
            })? {
                ("kept", reason)
            } else {
                report.bytes_eligible += bytes;
                if dry_run {
                    ("would_remove", "owned final session older than retention")
                } else {
                    file.remove().map_err(|error| {
                        invalid(format!("remove engine log {}: {error}", path.display()))
                    })?;
                    report.bytes_removed += bytes;
                    ("removed", "owned final session older than retention")
                }
            };
            tracing::info!(path = %path.display(), action, reason, bytes, "engine session retention decision");
            report.entries.push(FileDecision {
                path: path.clone(),
                bytes,
                action,
                reason,
            });
        }
        let mut value = serde_json::to_value(&report).map_err(invalid)?;
        value["retention_days"] = serde_json::json!(days);
        value["owned_sessions"] = serde_json::json!(ownership.len());
        value["unknown_paths"] = serde_json::json!(
            ownership
                .iter()
                .filter(|record| record.log_paths.is_empty())
                .count()
        );
        value["unavailable_paths"] = serde_json::json!(unavailable_paths);
        value["scan_after"] = serde_json::json!(scan_after.filter(|_| report.truncated));
        Ok(value)
    }
}

fn owned_paths(
    ownership: &[EngineSessionOwnership],
) -> BTreeMap<&std::path::PathBuf, Vec<&EngineSessionOwnership>> {
    let mut paths: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for record in ownership {
        for path in record.log_paths.iter().collect::<HashSet<_>>() {
            paths.entry(path).or_default().push(record);
        }
    }
    paths
}

fn admission(
    record: &EngineSessionOwnership,
    path: &std::path::Path,
    owner: Option<&Agent>,
    references: &HashMap<&str, Vec<aether_core::id::AgentId>>,
    in_flight: &HashSet<aether_core::id::AgentId>,
    days: u16,
    now: SystemTime,
) -> Option<&'static str> {
    if !file::recognized_path(record, path) {
        return Some("unrecognized engine log path");
    }
    if in_flight.contains(&record.agent_id) {
        return Some("owner has an in-flight turn");
    }
    let updated_at = if let Some(owner) = owner {
        // Failed is wakeable in place, even if archived or temporarily blocked.
        // A durable successor is the permanent wake refusal.
        match owner.status.kind() {
            AgentStatusKind::Completed | AgentStatusKind::Cancelled => {}
            AgentStatusKind::Failed if owner.restarted_as.is_some() => {}
            _ => return Some("owner is live or resumable"),
        }
        owner
            .updated_at
            .max(owner.status_since.unwrap_or(owner.updated_at))
    } else if let Some(expired) = &record.expired_owner {
        // Only history expiry writes this evidence, in the transaction that
        // deletes the agent. Without its row, waking/restarting is impossible.
        if !matches!(
            expired.status,
            AgentStatusKind::Completed | AgentStatusKind::Failed | AgentStatusKind::Cancelled
        ) {
            return Some("expired owner was not a final native agent");
        }
        expired.updated_at
    } else {
        return Some("owner history unavailable; finality unknown");
    };
    if references
        .get(record.session_id.as_str())
        .is_some_and(|ids| ids.iter().any(|id| *id != record.agent_id))
    {
        return Some("session referenced by another agent");
    }
    if !file::old_enough(SystemTime::from(updated_at), days, now) {
        return Some("owner changed within retention");
    }
    None
}
