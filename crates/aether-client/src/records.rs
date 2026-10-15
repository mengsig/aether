use crate::{Client, ClientError, Result, unexpected};
use aether_core::artifact::Artifact;
use aether_core::checkpoint::Checkpoint;
use aether_core::event::Event;
use aether_core::id::{AgentId, ArtifactId, InvocationId, MemoryId, RunId, SnapshotId};
use aether_core::memory::MemoryRecord;
use aether_core::replay::ReplayFrame;
use aether_core::report::RunReport;
use aether_core::snapshot::{Invocation, InvocationSnapshot, MemoriesInContext};
use aether_core::transfer::{ExportSummary, ImportSummary};
use aether_protocol::{
    ArtifactQuery, Command, EventQuery, MemoryQuerySpec, Reply, RunListing, TranscriptEntry,
};
use std::path::PathBuf;

impl Client {
    pub async fn storage(&self, gc: bool, dry_run: bool) -> Result<serde_json::Value> {
        if !self.welcome().is_some_and(|welcome| {
            welcome
                .features
                .iter()
                .any(|feature| feature == aether_protocol::features::STORAGE)
        }) {
            return Err(ClientError::Incompatible(
                "daemon does not support storage maintenance; update Aether".into(),
            ));
        }
        let command = if gc {
            Command::StorageGc { dry_run }
        } else {
            Command::StorageStatus
        };
        match self.request(command).await? {
            Reply::StorageReport(report) => Ok(report),
            other => Err(unexpected("storage report", &other)),
        }
    }

    pub async fn preview_retention(&self) -> Result<aether_core::retention::RetentionPreview> {
        match self.request(Command::PreviewRetention).await? {
            Reply::RetentionPreview(preview) => Ok(preview),
            other => Err(unexpected("retention preview", &other)),
        }
    }

    pub async fn runs(&self, limit: usize) -> Result<Vec<RunListing>> {
        match self.request(Command::ListRuns { limit }).await? {
            Reply::Runs(r) => Ok(r),
            other => Err(unexpected("runs", &other)),
        }
    }

    pub async fn run_report(&self, run: RunId) -> Result<RunReport> {
        match self.request(Command::GetRunReport { run }).await? {
            Reply::RunReport(r) => Ok(*r),
            other => Err(unexpected("run report", &other)),
        }
    }

    pub async fn replay(&self, agent: AgentId) -> Result<Vec<ReplayFrame>> {
        match self.request(Command::Replay { agent }).await? {
            Reply::Replay(frames) => Ok(frames),
            other => Err(unexpected("replay", &other)),
        }
    }

    pub async fn export_run(&self, run: RunId, path: PathBuf) -> Result<ExportSummary> {
        match self.request(Command::ExportRun { run, path }).await? {
            Reply::RunExported(s) => Ok(s),
            other => Err(unexpected("run export", &other)),
        }
    }

    pub async fn import_run(&self, path: PathBuf) -> Result<ImportSummary> {
        match self.request(Command::ImportRun { path }).await? {
            Reply::RunImported(s) => Ok(s),
            other => Err(unexpected("run import", &other)),
        }
    }

    pub async fn events(&self, after: u64, limit: usize, query: EventQuery) -> Result<Vec<Event>> {
        match self
            .request(Command::GetEvents {
                after,
                limit,
                query,
            })
            .await?
        {
            Reply::Events(e) => Ok(e),
            other => Err(unexpected("events", &other)),
        }
    }

    pub async fn transcript(&self, agent: AgentId, after: u64) -> Result<Vec<TranscriptEntry>> {
        match self
            .request(Command::GetTranscript { agent, after })
            .await?
        {
            Reply::Transcript(t) => Ok(t),
            other => Err(unexpected("transcript", &other)),
        }
    }

    pub async fn get_snapshot(&self, id: SnapshotId) -> Result<InvocationSnapshot> {
        match self.request(Command::GetSnapshot { id }).await? {
            Reply::Snapshot(s) => Ok(s),
            other => Err(unexpected("snapshot", &other)),
        }
    }

    pub async fn get_invocation(&self, id: InvocationId) -> Result<Invocation> {
        match self.request(Command::GetInvocation { id }).await? {
            Reply::Invocation(i) => Ok(i),
            other => Err(unexpected("invocation", &other)),
        }
    }

    pub async fn list_invocations(&self, agent: AgentId) -> Result<Vec<Invocation>> {
        match self.request(Command::ListInvocations { agent }).await? {
            Reply::Invocations(i) => Ok(i),
            other => Err(unexpected("invocations", &other)),
        }
    }

    pub async fn latest_checkpoint(&self, agent: AgentId) -> Result<Option<Checkpoint>> {
        match self.request(Command::LatestCheckpoint { agent }).await? {
            Reply::Checkpoint(c) => Ok(c),
            other => Err(unexpected("checkpoint", &other)),
        }
    }

    /// Publish bounded UTF-8 task evidence through the daemon's artifact store.
    pub async fn publish_artifact(
        &self,
        req: aether_protocol::PublishArtifact,
    ) -> Result<Artifact> {
        if !self.welcome().is_some_and(|w| {
            w.features
                .iter()
                .any(|f| f == aether_protocol::features::PUBLISH_ARTIFACT)
        }) {
            return Err(ClientError::Incompatible(
                "daemon does not support artifact publication; update Aether".into(),
            ));
        }
        match self.request(Command::PublishArtifact(req)).await? {
            Reply::Artifact(artifact) => Ok(artifact),
            other => Err(unexpected("artifact", &other)),
        }
    }

    pub async fn list_artifacts(&self, query: ArtifactQuery) -> Result<Vec<Artifact>> {
        match self.request(Command::ListArtifacts { query }).await? {
            Reply::Artifacts(a) => Ok(a),
            other => Err(unexpected("artifacts", &other)),
        }
    }

    /// Read artifact bytes (base64 on the wire, decoded here is left to the
    /// caller so large payloads are not copied twice).
    pub async fn read_artifact(&self, id: ArtifactId, max_bytes: usize) -> Result<Reply> {
        match self
            .request(Command::ReadArtifact { id, max_bytes })
            .await?
        {
            r @ Reply::ArtifactBytes { .. } => Ok(r),
            other => Err(unexpected("artifact_bytes", &other)),
        }
    }

    pub async fn get_artifact(&self, id: ArtifactId) -> Result<Artifact> {
        match self.request(Command::GetArtifact { id }).await? {
            Reply::Artifact(a) => Ok(a),
            other => Err(unexpected("artifact", &other)),
        }
    }

    /// Artifact bytes, decoded, with whether the payload was cut short.
    pub async fn artifact_bytes(
        &self,
        id: ArtifactId,
        max_bytes: usize,
    ) -> Result<(Artifact, Vec<u8>, bool)> {
        match self.read_artifact(id, max_bytes).await? {
            Reply::ArtifactBytes {
                artifact,
                base64,
                truncated,
            } => {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(base64)
                    .map_err(|e| ClientError::Malformed(format!("artifact bytes: {e}")))?;
                Ok((artifact, bytes, truncated))
            }
            other => Err(unexpected("artifact_bytes", &other)),
        }
    }

    /// A daemon that cannot filter by recorder would answer with every
    /// memory, which reads as all of it being theirs, so that is refused here.
    pub async fn memories(&self, query: MemoryQuerySpec) -> Result<Vec<MemoryRecord>> {
        if query.proposed {
            self.require_memory_proposals()?;
        }
        if query.recorded_by.is_some()
            && !self.welcome().is_some_and(|w| {
                w.features
                    .iter()
                    .any(|f| f == aether_protocol::features::MEMORY_RECORDED_BY)
            })
        {
            return Err(ClientError::Incompatible(
                "daemon cannot filter memories by recorder; update Aether".into(),
            ));
        }
        match self.request(Command::Memories { query }).await? {
            Reply::Memories(m) => Ok(m),
            other => Err(unexpected("memories", &other)),
        }
    }

    pub async fn create_memory(
        &self,
        spec: aether_core::memory::OperatorMemorySpec,
    ) -> Result<MemoryRecord> {
        if spec.proposed {
            self.require_memory_proposals()?;
        }
        match self
            .request(Command::CreateMemory {
                spec,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Memory(record) => Ok(record),
            other => Err(unexpected("memory", &other)),
        }
    }

    pub async fn accept_memory(
        &self,
        id: MemoryId,
        reason: Option<String>,
    ) -> Result<MemoryRecord> {
        self.require_memory_proposals()?;
        match self.request(Command::AcceptMemory { id, reason }).await? {
            Reply::Memory(record) => Ok(record),
            other => Err(unexpected("memory", &other)),
        }
    }

    pub async fn reject_memory(&self, id: MemoryId, reason: String) -> Result<MemoryRecord> {
        self.require_memory_proposals()?;
        match self.request(Command::RejectMemory { id, reason }).await? {
            Reply::Memory(record) => Ok(record),
            other => Err(unexpected("memory", &other)),
        }
    }

    fn require_memory_proposals(&self) -> Result<()> {
        if self.welcome().is_some_and(|welcome| {
            welcome
                .features
                .iter()
                .any(|feature| feature == aether_protocol::features::MEMORY_PROPOSALS)
        }) {
            return Ok(());
        }
        Err(ClientError::Incompatible(
            "daemon cannot review memory proposals; update Aether".into(),
        ))
    }

    pub async fn forget_memory(
        &self,
        id: MemoryId,
        reason: Option<String>,
    ) -> Result<MemoryRecord> {
        match self.request(Command::ForgetMemory { id, reason }).await? {
            Reply::Memory(m) => Ok(m),
            other => Err(unexpected("memory", &other)),
        }
    }

    /// Put a forgotten memory back.
    pub async fn restore_memory(&self, id: MemoryId) -> Result<MemoryRecord> {
        match self.request(Command::RestoreMemory { id }).await? {
            Reply::Memory(m) => Ok(m),
            other => Err(unexpected("memory", &other)),
        }
    }

    pub async fn pin_memory(&self, id: MemoryId, pinned: bool) -> Result<MemoryRecord> {
        match self.request(Command::PinMemory { id, pinned }).await? {
            Reply::Memory(m) => Ok(m),
            other => Err(unexpected("memory", &other)),
        }
    }

    /// Alias `alias`, an old repository id, to the repository at `root` or to
    /// the id `repository`, moving every memory filed under it. A dry run
    /// reports what would move and moves nothing.
    pub async fn alias_repository(
        &self,
        alias: String,
        root: Option<std::path::PathBuf>,
        repository: Option<String>,
        dry_run: bool,
    ) -> Result<aether_core::repository::RepositoryAliasReport> {
        self.require_repository_aliases()?;
        let command = Command::AliasRepository {
            alias,
            root,
            repository,
            dry_run,
        };
        match self.request(command).await? {
            Reply::RepositoryAliased(report) => Ok(report),
            other => Err(unexpected("repository aliased", &other)),
        }
    }

    /// Every registered repository alias, ordered by alias.
    pub async fn repository_aliases(
        &self,
    ) -> Result<Vec<aether_core::repository::RepositoryAlias>> {
        self.require_repository_aliases()?;
        match self.request(Command::ListRepositoryAliases).await? {
            Reply::RepositoryAliases(aliases) => Ok(aliases),
            other => Err(unexpected("repository aliases", &other)),
        }
    }

    /// Which stored memories each model call of `task`, or `invocation`, was
    /// given. Name exactly one; the daemon refuses otherwise.
    pub async fn memories_in_context(
        &self,
        task: Option<aether_core::id::TaskId>,
        invocation: Option<InvocationId>,
    ) -> Result<MemoriesInContext> {
        let served = self.welcome().is_some_and(|w| {
            w.features
                .iter()
                .any(|f| f == aether_protocol::features::MEMORY_CONTEXT)
        });
        if !served {
            return Err(ClientError::Incompatible(
                "daemon cannot say which memories were in context; update Aether".into(),
            ));
        }
        match self
            .request(Command::MemoriesInContext { task, invocation })
            .await?
        {
            Reply::MemoriesInContext(found) => Ok(found),
            other => Err(unexpected("memories in context", &other)),
        }
    }

    fn require_repository_aliases(&self) -> Result<()> {
        let served = self.welcome().is_some_and(|w| {
            w.features
                .iter()
                .any(|f| f == aether_protocol::features::REPOSITORY_ALIASES)
        });
        if served {
            return Ok(());
        }
        Err(ClientError::Incompatible(
            "daemon cannot alias repositories; update Aether".into(),
        ))
    }

    /// Read a repository's committed notes into memory scoped to it.
    pub async fn sync_repository_knowledge(
        &self,
        root: Option<std::path::PathBuf>,
    ) -> Result<(String, aether_core::memory::SeedReport)> {
        match self
            .request(Command::SyncRepositoryKnowledge { root })
            .await?
        {
            Reply::MemorySeeded { repository, report } => Ok((repository, report)),
            other => Err(unexpected("memory seeded", &other)),
        }
    }
}
