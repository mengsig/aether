use aether_core::{
    id::RunId,
    retention::{RetentionPreview, RunRetentionDecision},
    time::Timestamp,
};
use aether_store::{AgentFilter, RunExpiry};
use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::{Result, Runtime};
mod worker;
pub(crate) use worker::spawn;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct HistorySweep {
    pub interrupted: bool,
    pub expired_runs: usize,
    pub expired_events: usize,
    pub expired_rows: usize,
    pub protected: BTreeMap<&'static str, usize>,
}

impl Runtime {
    /// Preview current history and artifact decisions without applying any mutation.
    /// Simulate ordered expiry on an isolated snapshot; this is not a reservation.
    pub fn preview_retention(&self) -> Result<RetentionPreview> {
        self.preview_retention_at(aether_core::time::now())
    }

    fn preview_retention_at(&self, at: Timestamp) -> Result<RetentionPreview> {
        let days = self
            .models
            .config()
            .storage
            .event_history_days
            .unwrap_or(self.models.config().retention.completed_run_days.get());
        let before = at - chrono::Duration::days(i64::from(days));
        let shadow = self.store.retention_snapshot()?;
        let gate = aether_store::WriteGate::new();
        let runs: BTreeSet<_> = shadow
            .agents(&AgentFilter::default())?
            .into_iter()
            .map(|agent| agent.run)
            .collect();
        let mut report = RetentionPreview {
            at,
            history_before: before,
            runs: vec![],
            eligible_artifacts: vec![],
            artifact_payload_bytes: 0,
            history_payload_bytes: 0,
            pending_cleanup: Default::default(),
        };
        for run in runs.into_iter().filter(|_| days > 0) {
            let Some(mut preview) = shadow.preview_run_history(run, before)? else {
                continue;
            };
            if preview.decision == RunRetentionDecision::Eligible {
                let _hold = self.hold();
                let agents: HashSet<_> = shadow
                    .agents(&AgentFilter {
                        run: Some(run),
                        ..Default::default()
                    })?
                    .into_iter()
                    .map(|agent| agent.id)
                    .collect();
                let sched = self.sched.lock();
                if let Some(reason) = self.runtime_expiry_blocker(&agents, &sched) {
                    preview.decision = RunRetentionDecision::Protected {
                        reason: reason.into(),
                    };
                    preview.events = 0;
                    preview.rows = 0;
                    preview.history_payload_bytes = 0;
                }
            }
            if preview.decision == RunRetentionDecision::Eligible {
                match shadow.expire_run_history(run, before, &gate.hold())? {
                    RunExpiry::Expired { .. } => {}
                    other => {
                        return Err(crate::RuntimeError::Internal(format!(
                            "retention snapshot expiry disagreed with preview: {other:?}"
                        )));
                    }
                }
            }
            report.history_payload_bytes += preview.history_payload_bytes;
            report.runs.push(preview);
        }
        let _hold = futures::executor::block_on(self.artifact_lock.lock());
        let live = self.store.live_artifacts()?;
        let doomed = self.config.retention.evictable(&live, at);
        let mut blobs = HashSet::new();
        for artifact in doomed {
            report.eligible_artifacts.push(artifact.id);
            let key = artifact
                .digest
                .clone()
                .unwrap_or_else(|| artifact.id.to_string());
            if blobs.insert(key) {
                report.artifact_payload_bytes += artifact.size_bytes;
            }
        }
        report.pending_cleanup = self.store.pending_artifact_cleanup()?;
        Ok(report)
    }

    /// Apply the configured completed-run age. Each run is its own atomic
    /// transaction: a later error does not undo runs already expired.
    pub fn sweep_run_history(&self) -> Result<HistorySweep> {
        self.sweep_run_history_at(aether_core::time::now())
    }

    fn sweep_run_history_at(&self, at: Timestamp) -> Result<HistorySweep> {
        self.sweep_run_history_inner(at, false)
    }

    fn sweep_run_history_inner(
        &self,
        at: Timestamp,
        stop_on_shutdown: bool,
    ) -> Result<HistorySweep> {
        let days = self
            .models
            .config()
            .storage
            .event_history_days
            .unwrap_or(self.models.config().retention.completed_run_days.get());
        let before = at - chrono::Duration::days(i64::from(days));
        // This is only candidate discovery. Expiry re-reads every agent and
        // task under the write gate and checks durable references atomically.
        let runs: BTreeSet<_> = self
            .store
            .agents(&AgentFilter::default())?
            .into_iter()
            .filter(|agent| agent.updated_at < before && agent.status.is_terminal())
            .map(|agent| agent.run)
            .collect();
        let mut report = HistorySweep::default();
        for run in runs.into_iter().filter(|_| days > 0) {
            if stop_on_shutdown && self.shutdown.is_cancelled() {
                report.interrupted = true;
                break;
            }
            match self.expire_run_history(run, before)? {
                RunExpiry::Missing => {}
                RunExpiry::Protected(reason) => *report.protected.entry(reason).or_default() += 1,
                RunExpiry::Expired { events, rows } => {
                    report.expired_runs += 1;
                    report.expired_events += events;
                    report.expired_rows += rows;
                    tracing::info!(%run, events, rows, "expired completed-run history");
                }
            }
        }
        Ok(report)
    }

    /// Expire one old finished run without racing a state change or a turn
    /// still reporting its final usage. This is not an automatic sweep.
    pub fn expire_run_history(&self, run: RunId, before: Timestamp) -> Result<RunExpiry> {
        // Judging a run scans whole tables (minutes per sweep on a large store), and most
        // old runs stay protected; doing it under the gate and `sched` stalled every commit and `status`.
        if let Some(kept) = self.store.screen_run_expiry(run, before)? {
            return Ok(kept);
        }
        let hold = self.hold();
        let agents = self.store.agents(&AgentFilter {
            run: Some(run),
            ..Default::default()
        })?;
        let agents: HashSet<_> = agents.iter().map(|agent| agent.id).collect();
        let mut sched = self.sched.lock();
        if let Some(reason) = self.runtime_expiry_blocker(&agents, &sched) {
            return Ok(RunExpiry::Protected(reason));
        }
        let result = self.store.expire_run_history(run, before, &hold)?;
        if matches!(result, RunExpiry::Expired { .. }) {
            sched.ready_since.retain(|id, _| !agents.contains(id));
            sched
                .entry_refused_until
                .retain(|id, _| !agents.contains(id));
            self.seeded_repositories
                .lock()
                .retain(|id| !agents.contains(id));
        }
        // Keep admission locked until deletion commits. A stale scheduling
        // candidate must not enter while its durable history is being removed.
        drop(sched);
        Ok(result)
    }

    fn runtime_expiry_blocker(
        &self,
        agents: &HashSet<aether_core::id::AgentId>,
        sched: &crate::SchedulerState,
    ) -> Option<&'static str> {
        if agents.iter().any(|id| sched.running.contains_key(id)) {
            return Some("in-flight turn");
        }
        let engine_turns = self.engine_turns.lock();
        if agents.iter().any(|id| engine_turns.contains_key(id)) {
            return Some("native engine turn");
        }
        drop(engine_turns);
        if self
            .resources
            .holdings()
            .iter()
            .any(|holding| agents.contains(&holding.agent))
        {
            return Some("held resource");
        }
        None
    }
}

#[cfg(test)]
mod tests;
