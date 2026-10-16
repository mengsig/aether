//! Test doubles shared by the crate's unit tests.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aether_core::agent::Scope;
use aether_core::artifact::{Artifact, ArtifactKind};
use aether_core::event::Actor;
use aether_core::id::{AgentId, ArtifactId, TaskId};
use async_trait::async_trait;

use crate::tool::{ArtifactSink, ToolContext, ToolError};

/// In-memory artifact sink recording every stored payload.
#[derive(Default)]
pub struct MemorySink {
    stored: Mutex<Vec<(Artifact, Vec<u8>)>>,
}

impl MemorySink {
    pub fn stored(&self) -> Vec<(Artifact, Vec<u8>)> {
        self.stored.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl ArtifactSink for MemorySink {
    async fn store(
        &self,
        kind: ArtifactKind,
        name: &str,
        summary: &str,
        content_type: Option<&str>,
        bytes: Vec<u8>,
        ctx_agent: AgentId,
        ctx_task: Option<TaskId>,
    ) -> Result<Artifact, ToolError> {
        let artifact = Artifact {
            id: ArtifactId::new(),
            kind,
            name: name.to_owned(),
            path: None,
            summary: summary.to_owned(),
            size_bytes: bytes.len() as u64,
            content_type: content_type.map(str::to_owned),
            digest: None,
            created_by: Actor::agent(ctx_agent),
            agent_id: Some(ctx_agent),
            task_id: ctx_task,
            created_at: aether_core::time::now(),
            evicted_at: None,
        };
        self.stored
            .lock()
            .map_err(|_| ToolError::Internal("sink poisoned".into()))?
            .push((artifact.clone(), bytes));
        Ok(artifact)
    }
}

/// Bounds a hang in a test that waits on a real process; it is not an expected
/// duration. These tests start `bash -l`, which a loaded host can take seconds
/// just to get through the login profile.
pub const WAIT_CEILING: std::time::Duration = std::time::Duration::from_secs(60);

/// What one poll of a wait condition saw.
pub enum Observed {
    Held,
    /// Not yet; reported if the wait reaches the ceiling.
    Pending(String),
    /// It can never hold now: report the cause instead of waiting it out.
    Impossible(String),
}

/// Poll until `observe` reports the condition held, or fail naming `what` and
/// the last thing seen.
pub async fn wait_for(what: &str, mut observe: impl FnMut() -> Observed) {
    let deadline = std::time::Instant::now() + WAIT_CEILING;
    loop {
        match observe() {
            Observed::Held => return,
            Observed::Impossible(why) => panic!("waiting for {what}: {why}"),
            Observed::Pending(seen) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "waiting for {what}: never held within {WAIT_CEILING:?}; last saw {seen}"
                );
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// A context whose workspace root and cwd are `root`.
pub fn ctx_in(root: &Path) -> ToolContext {
    ToolContext {
        agent_id: AgentId::new(),
        task_id: None,
        instance_id: "test-instance".into(),
        scope: Scope {
            workspace_root: Some(root.to_path_buf()),
            ..Default::default()
        },
        cwd: root.to_path_buf(),
        artifacts: Arc::new(MemorySink::default()),
        env: BTreeMap::new(),
        hidden_env: Vec::new().into(),
        temp_dir: None,
        build_env: Vec::new(),
    }
}
