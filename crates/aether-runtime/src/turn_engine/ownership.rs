use std::{path::PathBuf, sync::Arc};

use aether_core::{id::AgentId, time::now};
use aether_engine::{EngineKind, SessionRecorder};
use aether_store::{Commit, EngineSessionOwnership, Mutation, Store};

struct OwnershipRecorder {
    store: Arc<dyn Store>,
    agent_id: AgentId,
}

impl std::fmt::Debug for OwnershipRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnershipRecorder")
            .field("agent_id", &self.agent_id)
            .finish_non_exhaustive()
    }
}

impl SessionRecorder for OwnershipRecorder {
    fn record(&self, engine: EngineKind, id: &str, log_paths: Vec<PathBuf>) -> Result<(), String> {
        self.store
            .commit(Commit::new().mutate(Mutation::RecordEngineSessionOwnership(
                EngineSessionOwnership {
                    engine: engine.as_str().into(),
                    session_id: id.into(),
                    agent_id: self.agent_id,
                    log_paths,
                    created_at: now(),
                    expired_owner: None,
                },
            )))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub(crate) fn session_recorder(
    runtime: &super::Runtime,
    agent_id: AgentId,
) -> Arc<dyn SessionRecorder> {
    Arc::new(OwnershipRecorder {
        store: runtime.store.clone(),
        agent_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ownership_recorder_preserves_agent_identity_without_an_agent_row() {
        let (runtime, _dir) = crate::test_runtime(|_| {}).await;
        let agent = AgentId::new();
        let recorder = session_recorder(&runtime, agent);
        recorder
            .record(EngineKind::CODEX, "new-thread", Vec::new())
            .unwrap();
        let records = runtime.store.engine_session_ownerships().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].engine, "codex");
        assert_eq!(records[0].session_id, "new-thread");
        assert_eq!(records[0].agent_id, agent);
        assert!(runtime.store.agent(agent).unwrap().is_none());
        let refused = recorder
            .record(EngineKind::new("unowned-engine"), "id", Vec::new())
            .unwrap_err();
        assert!(refused.contains("unowned-engine"), "{refused}");
        assert!(!refused.contains("corrupt"), "{refused}");
        assert_eq!(runtime.store.engine_session_ownerships().unwrap(), records);
        runtime.shutdown().await;
    }

    /// A real `agy acp` 1.5.18 `session/new` id: every antigravity task
    /// failed after its first turn while the store refused this shape.
    #[tokio::test]
    async fn ownership_recorder_accepts_a_real_antigravity_session() {
        let (runtime, _dir) = crate::test_runtime(|_| {}).await;
        let agent = AgentId::new();
        let recorder = session_recorder(&runtime, agent);
        let session = "e595a646-998a-44b5-b978-759aa2cfabf1";
        recorder
            .record(EngineKind::ANTIGRAVITY, session, Vec::new())
            .unwrap();
        let records = runtime.store.engine_session_ownerships().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            (records[0].engine.as_str(), records[0].session_id.as_str()),
            ("antigravity", session)
        );
        assert_eq!(records[0].agent_id, agent);
        runtime.shutdown().await;
    }
}
