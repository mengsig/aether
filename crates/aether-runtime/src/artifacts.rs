//! Artifact storage for tools: bytes go to the store, the record becomes an event.
//!
//! A tool's output is masked by the runtime redactor before it is stored, so
//! its secrets never reach the store. What a client publishes is kept as sent
//! and masked where it is read instead.

use std::sync::Arc;

use aether_core::artifact::{Artifact, ArtifactKind};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, ArtifactId, RunId, TaskId};
use aether_core::time::now;
use aether_store::{Commit, Mutation, StagedArtifact};
use aether_tools::{ArtifactSink, ToolError};

use crate::Runtime;

pub struct StoreArtifactSink {
    pub runtime: Arc<Runtime>,
}

impl std::fmt::Debug for StoreArtifactSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoreArtifactSink")
    }
}

#[async_trait::async_trait]
impl ArtifactSink for StoreArtifactSink {
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
        let bytes = match self.runtime.tool_runtime.redactor().redact_bytes(&bytes) {
            std::borrow::Cow::Borrowed(_) => bytes,
            std::borrow::Cow::Owned(clean) => clean,
        };
        self.runtime
            .publish_artifact(
                ArtifactPublication {
                    kind,
                    name: name.into(),
                    summary: summary.into(),
                    content_type: content_type.map(str::to_owned),
                    bytes,
                    agent: ctx_agent,
                    task: ctx_task,
                },
                Actor::agent(ctx_agent),
            )
            .await
    }

    async fn store_file(
        &self,
        kind: ArtifactKind,
        name: &str,
        summary: &str,
        content_type: Option<&str>,
        source: &std::path::Path,
        ctx_agent: AgentId,
        ctx_task: Option<TaskId>,
    ) -> Result<Artifact, ToolError> {
        let runtime = self.runtime.clone();
        let source = source.to_path_buf();
        let name = name.to_owned();
        let summary = summary.to_owned();
        let content_type = content_type.map(str::to_owned);
        tokio::task::spawn_blocking(move || {
            // Masked before the lock: a large log must not stall every other publication.
            let masked = masked_copy(runtime.tool_runtime.redactor(), &source)?;
            let _hold = runtime.artifact_lock.blocking_lock();
            let run = run_of(&runtime, ctx_agent)?;
            let id = ArtifactId::new();
            let imported = runtime
                .store
                .import_artifact_file(id, masked.path())
                .map_err(|e| ToolError::Internal(e.to_string()))?;
            let artifact = Artifact {
                id,
                kind,
                name,
                path: Some(imported.path),
                summary,
                size_bytes: imported.size_bytes,
                content_type,
                digest: Some(imported.digest.clone()),
                created_by: Actor::agent(ctx_agent),
                agent_id: Some(ctx_agent),
                task_id: ctx_task,
                created_at: now(),
                evicted_at: None,
            };
            let event = match imported.shared_with {
                Some(shared_with) => EventKind::ArtifactDeduplicated {
                    artifact: id,
                    shared_with: Some(shared_with),
                    digest: imported.digest,
                },
                None => EventKind::ArtifactCreated {
                    artifact: artifact.clone(),
                },
            };
            let mut draft = EventDraft::new(Actor::agent(ctx_agent), event).agent(ctx_agent);
            draft.task_id = ctx_task;
            draft.run = run;
            commit_artifact(&runtime, artifact, draft, imported.publication)
        })
        .await
        .map_err(|e| ToolError::Internal(e.to_string()))?
    }
}

/// A masked copy of `source`, beside it so it stays on the same disk and
/// under the same owner. The raw file stays where it was for its producer.
fn masked_copy(
    redactor: &aether_tools::Redactor,
    source: &std::path::Path,
) -> Result<tempfile::NamedTempFile, ToolError> {
    let dir = source
        .parent()
        .ok_or_else(|| ToolError::Internal(format!("{} has no directory", source.display())))?;
    let input = std::fs::File::open(source)?;
    let mut masked = tempfile::NamedTempFile::new_in(dir)?;
    redactor.redact_stream(std::io::BufReader::new(input), masked.as_file_mut())?;
    Ok(masked)
}

/// Payload and association for an artifact produced by a tool or client.
#[derive(Debug)]
pub struct ArtifactPublication {
    pub kind: ArtifactKind,
    pub name: String,
    pub summary: String,
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
    pub agent: AgentId,
    pub task: Option<TaskId>,
}

impl Runtime {
    pub async fn publish_artifact(
        &self,
        publication: ArtifactPublication,
        by: Actor,
    ) -> Result<Artifact, ToolError> {
        let ArtifactPublication {
            kind,
            name,
            summary,
            content_type,
            bytes,
            agent: ctx_agent,
            task: ctx_task,
        } = publication;
        let run = run_of(self, ctx_agent)?;
        let digest = {
            use sha2::Digest as _;
            hex::encode(sha2::Sha256::digest(&bytes))
        };
        // The same content is stored once, but every producing call gets its
        // own record: a task's result must list what that task produced, and
        // the payload must age by the newest reference, not the oldest twin.
        let hold = self.artifact_lock.lock().await;
        if let Some(twin) = self
            .store
            .artifact_by_digest(&digest)
            .map_err(|e| ToolError::Internal(e.to_string()))?
            && let Some(path) = twin.path.clone()
            && self
                .store
                .artifact_bytes_exist(twin.id)
                .map_err(|e| ToolError::Internal(e.to_string()))?
        {
            let artifact = Artifact {
                id: ArtifactId::new(),
                kind,
                name: name.to_owned(),
                path: Some(path),
                summary: summary.to_owned(),
                size_bytes: bytes.len() as u64,
                content_type: content_type.clone(),
                digest: Some(digest.clone()),
                created_by: by.clone(),
                agent_id: Some(ctx_agent),
                task_id: ctx_task,
                created_at: now(),
                evicted_at: None,
            };
            let mut draft = EventDraft::new(
                by.clone(),
                EventKind::ArtifactDeduplicated {
                    artifact: artifact.id,
                    shared_with: Some(twin.id),
                    digest,
                },
            )
            .agent(ctx_agent);
            draft.task_id = ctx_task;
            draft.run = run;
            return commit_artifact(self, artifact, draft, None);
        }
        drop(hold);
        let id = ArtifactId::new();
        let size = bytes.len() as u64;
        let store = self.store.clone();
        let publication =
            tokio::task::spawn_blocking(move || store.stage_artifact_bytes(id, &bytes))
                .await
                .map_err(|e| ToolError::Internal(e.to_string()))?
                .map_err(|e| ToolError::Internal(e.to_string()))?;
        let _hold = self.artifact_lock.lock().await;
        let artifact = Artifact {
            id,
            kind,
            name: name.to_owned(),
            path: Some(publication.path.clone()),
            summary: summary.to_owned(),
            size_bytes: size,
            content_type: content_type.clone(),
            digest: Some(digest),
            created_by: by.clone(),
            agent_id: Some(ctx_agent),
            task_id: ctx_task,
            created_at: now(),
            evicted_at: None,
        };
        let mut draft = EventDraft::new(
            by.clone(),
            EventKind::ArtifactCreated {
                artifact: artifact.clone(),
            },
        )
        .agent(ctx_agent);
        draft.task_id = ctx_task;
        draft.run = run;
        commit_artifact(self, artifact, draft, Some(publication))
    }
}

fn commit_artifact(
    runtime: &Runtime,
    artifact: Artifact,
    draft: EventDraft,
    publication: Option<StagedArtifact>,
) -> Result<Artifact, ToolError> {
    if let Err(error) = runtime.apply(
        Commit::new()
            .event(draft)
            .mutate(Mutation::PutArtifact(artifact.clone())),
    ) {
        if publication.is_some()
            && let Err(cleanup) = runtime
                .store
                .remove_unreferenced_artifact_bytes(artifact.id)
        {
            return Err(ToolError::Internal(format!(
                "{error}; cleaning unpublished artifact {} also failed: {cleanup}",
                artifact.id
            )));
        }
        return Err(ToolError::Internal(error.to_string()));
    }
    if let Some(publication) = publication
        && let Err(error) = runtime.store.finish_artifact_publication(&publication)
    {
        tracing::warn!(artifact = %artifact.id, %error,
            "artifact payload promotion failed; will retry on next open");
    }
    Ok(artifact)
}

/// The run an artifact belongs to. Without it the artifact's event sits
/// outside the run's log, and a report or an export of that run cannot see
/// what the run produced.
fn run_of(runtime: &Runtime, agent: AgentId) -> Result<Option<RunId>, ToolError> {
    Ok(runtime
        .store
        .agent(agent)
        .map_err(|e| ToolError::Internal(e.to_string()))?
        .map(|a| a.run))
}

impl Runtime {
    /// Drop the bytes of artifacts the retention policy no longer keeps. The
    /// records stay, so an old task's report still names what it produced and
    /// says the payload is gone. Returns how many were evicted.
    pub fn sweep_artifacts(&self) -> crate::Result<usize> {
        let policy = self.config.retention;
        if policy == aether_core::artifact::RetentionPolicy::keep_everything() {
            return Ok(0);
        }
        // Startup calls this synchronous API from Tokio, where blocking_lock would panic.
        let _hold = futures::executor::block_on(self.artifact_lock.lock());
        let live = self.store.live_artifacts()?;
        let at = now();
        let doomed: Vec<Artifact> = policy.evictable(&live, at).into_iter().cloned().collect();
        let reason = describe_policy(&policy);
        for artifact in &doomed {
            self.evict_with_event(artifact, &reason, at)?;
        }
        Ok(doomed.len())
    }
}

impl Runtime {
    /// The record's eviction and its `ArtifactEvicted` event commit together,
    /// so a failed commit leaves the artifact live for the next sweep. The
    /// payload unlink follows; a failed unlink stays pending and is retried
    /// when the store opens.
    fn evict_with_event(
        &self,
        artifact: &Artifact,
        reason: &str,
        at: aether_core::time::Timestamp,
    ) -> crate::Result<()> {
        if artifact.evicted_at.is_some() {
            return Err(crate::RuntimeError::Invalid(format!(
                "Artifact {} is already evicted; run `aether doctor` to inspect retention state",
                artifact.id
            )));
        }
        // On the run's log, so a report of that run still says the
        // payload is gone rather than losing the record of it.
        let run = match artifact.agent_id {
            Some(a) => self.store.agent(a)?.map(|a| a.run),
            None => None,
        };
        let mut draft = EventDraft::new(
            Actor::System,
            EventKind::ArtifactEvicted {
                artifact: artifact.id,
                size_bytes: artifact.size_bytes,
                reason: reason.to_owned(),
            },
        );
        draft.agent_id = artifact.agent_id;
        draft.run = run;
        let mut evicted = artifact.clone();
        evicted.evicted_at = Some(at);
        self.apply(
            Commit::new()
                .event(draft)
                .mutate(Mutation::PutArtifact(evicted)),
        )?;
        self.store.evict_artifact_bytes(artifact.id)?;
        Ok(())
    }
}

fn describe_policy(policy: &aether_core::artifact::RetentionPolicy) -> String {
    match (policy.max_age_days, policy.max_total_bytes) {
        (Some(days), Some(bytes)) => format!("retention: {days} days, {bytes} bytes"),
        (Some(days), None) => format!("retention: {days} days"),
        (None, Some(bytes)) => format!("retention: {bytes} bytes"),
        (None, None) => "retention".into(),
    }
}
