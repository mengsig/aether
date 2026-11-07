//! Memory 2.0's host commands (docs/memory/MEMORY-V2.md).

use aether_core::id::{InvocationId, TaskId};
use aether_core::memory::MemoryScope;
use aether_core::memory_v2_reports::{
    MemoryConsolidation, MemoryExport, MemoryImport, MemoryMigrationStatus, RetrievalExplain,
};
use aether_protocol::{Command, Reply};

use crate::{Client, ClientError, Result, unexpected};

impl Client {
    pub async fn memory_retrieval_explain(
        &self,
        task: Option<TaskId>,
        invocation: Option<InvocationId>,
    ) -> Result<RetrievalExplain> {
        self.require_memory_v2()?;
        if task.is_some() == invocation.is_some() {
            return Err(ClientError::InvalidUsage(
                "name exactly one of a task or an invocation".into(),
            ));
        }
        match self
            .request(Command::MemoryRetrievalExplain { task, invocation })
            .await?
        {
            Reply::MemoryRetrievalExplain(explain) => {
                if task.is_some_and(|task| explain.turns.iter().any(|t| t.task != Some(task))) {
                    return Err(ClientError::Malformed(
                        "daemon explained a turn of another task".into(),
                    ));
                }
                Ok(explain)
            }
            other => Err(unexpected("memory retrieval explain", &other)),
        }
    }

    pub async fn consolidate_memories(
        &self,
        scope: Option<MemoryScope>,
        dry_run: bool,
    ) -> Result<MemoryConsolidation> {
        self.require_memory_v2()?;
        match self
            .request(Command::ConsolidateMemories { scope, dry_run })
            .await?
        {
            Reply::MemoryConsolidation(report) if report.dry_run == dry_run => Ok(report),
            Reply::MemoryConsolidation(_) => Err(ClientError::Malformed(
                "daemon answered a different dry run".into(),
            )),
            other => Err(unexpected("memory consolidation", &other)),
        }
    }

    pub async fn memory_export(&self, scope: Option<MemoryScope>) -> Result<MemoryExport> {
        self.require_memory_v2()?;
        match self.request(Command::MemoryExport { scope }).await? {
            Reply::MemoryExport(export) => Ok(export),
            other => Err(unexpected("memory export", &other)),
        }
    }

    pub async fn memory_import(&self, artifact: String, dry_run: bool) -> Result<MemoryImport> {
        self.require_memory_v2()?;
        match self
            .request(Command::MemoryImport { artifact, dry_run })
            .await?
        {
            Reply::MemoryImport(report) if report.dry_run == dry_run => Ok(report),
            Reply::MemoryImport(_) => Err(ClientError::Malformed(
                "daemon answered a different dry run".into(),
            )),
            other => Err(unexpected("memory import", &other)),
        }
    }

    pub async fn memory_migration_status(&self) -> Result<MemoryMigrationStatus> {
        self.require_memory_v2()?;
        match self.request(Command::MemoryMigrationStatus).await? {
            Reply::MemoryMigrationStatus(status) => Ok(status),
            other => Err(unexpected("memory migration status", &other)),
        }
    }

    fn require_memory_v2(&self) -> Result<()> {
        if self.welcome().is_some_and(|welcome| {
            welcome
                .features
                .iter()
                .any(|feature| feature == aether_protocol::features::MEMORY_V2)
        }) {
            return Ok(());
        }
        Err(ClientError::Incompatible(
            "daemon does not serve Memory 2.0 commands; update Aether".into(),
        ))
    }
}
