//! What `aether status` and `aether doctor` list as needing attention.
//!
//! A problem about one agent or task lasts only while its subject can still
//! act: the scheduler drops it once the agent ends or the task leaves the
//! queue, so a cancelled agent's stall is not reported until a restart.

use std::collections::BTreeMap;

use aether_core::id::{AgentId, TaskId};

/// What a problem is about, when it is about one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Subject {
    Agent(AgentId),
    Task(TaskId),
}

#[derive(Debug)]
struct Problem {
    detail: String,
    subject: Option<Subject>,
}

#[derive(Debug, Default)]
pub(crate) struct Problems(parking_lot::RwLock<BTreeMap<String, Problem>>);

impl Problems {
    /// Logged once per change, so a step that keeps failing the same way
    /// does not flood the log.
    pub(crate) fn report(&self, key: String, detail: String, subject: Option<Subject>) {
        assert!(!key.is_empty(), "a problem needs a key to clear it by");
        let mut problems = self.0.write();
        let unchanged = problems
            .get(&key)
            .is_some_and(|known| known.detail == detail && known.subject == subject);
        if unchanged {
            return;
        }
        tracing::error!(%key, %detail);
        problems.insert(key, Problem { detail, subject });
    }

    pub(crate) fn clear(&self, key: &str) {
        self.0.write().remove(key);
    }

    pub(crate) fn details(&self) -> Vec<String> {
        self.0.read().values().map(|p| p.detail.clone()).collect()
    }

    /// Drop the problems whose subject `keep` rejects; daemon-wide ones stay.
    /// Returns how many were dropped.
    pub(crate) fn retain(&self, keep: impl Fn(Subject) -> bool) -> usize {
        let mut problems = self.0.write();
        let before = problems.len();
        problems.retain(|_, problem| problem.subject.is_none_or(&keep));
        let dropped = before - problems.len();
        debug_assert!(
            problems.values().all(|p| p.subject.is_none_or(&keep)),
            "a rejected subject's problem survived"
        );
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retain_drops_only_rejected_subjects_and_keeps_daemon_problems() {
        let problems = Problems::default();
        let (gone, live) = (AgentId::new(), AgentId::new());
        let task = TaskId::new();
        problems.report("disk.check".into(), "disk".into(), None);
        problems.report("a".into(), "gone".into(), Some(Subject::Agent(gone)));
        problems.report("b".into(), "live".into(), Some(Subject::Agent(live)));
        problems.report("c".into(), "task".into(), Some(Subject::Task(task)));
        let dropped = problems.retain(|subject| subject != Subject::Agent(gone));
        assert_eq!(dropped, 1);
        let mut left = problems.details();
        left.sort();
        assert_eq!(left, ["disk", "live", "task"]);
    }

    #[test]
    fn a_report_replaces_the_detail_and_subject_under_its_key() {
        let problems = Problems::default();
        let agent = AgentId::new();
        problems.report("k".into(), "first".into(), None);
        problems.report("k".into(), "second".into(), Some(Subject::Agent(agent)));
        assert_eq!(problems.details(), ["second"]);
        assert_eq!(problems.retain(|_| false), 1);
        assert!(problems.details().is_empty());
    }
}
