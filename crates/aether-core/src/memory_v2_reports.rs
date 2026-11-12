//! The Memory 2.0 command replies and the export artifact's lines
//! (docs/memory/MEMORY-V2.md §2).

use serde::{Deserialize, Serialize};

use crate::id::{AgentId, InvocationId, MemoryId, SnapshotId, TaskId};
use crate::memory::{MemoryRecord, MemoryScope};
use crate::memory_admission::MemoryAdmission;
use crate::memory_v2::{StaleNote, TraceCandidate, TraceOmitted, TracePolicy};
use crate::time::Timestamp;

/// What retrieval did for each model call of a task, or one invocation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RetrievalExplain {
    pub turns: Vec<ExplainedTurn>,
    /// Snapshots past the limit were left out.
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExplainedTurn {
    pub snapshot: SnapshotId,
    pub agent: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    pub turn: u32,
    pub at: Timestamp,
    #[serde(default)]
    pub invocations: Vec<InvocationId>,
    /// False when the snapshot predates the trace or the turn did not retrieve.
    pub traced: bool,
    /// The snapshot's memory layer, in render order: what the model was given.
    #[serde(default)]
    pub injected: Vec<MemoryId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<TracePolicy>,
    pub pool: u32,
    #[serde(default)]
    pub candidates: Vec<TraceCandidate>,
    #[serde(default)]
    pub omitted: TraceOmitted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryConsolidation {
    pub dry_run: bool,
    pub scanned: u32,
    pub clusters: Vec<ConsolidatedCluster>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConsolidatedCluster {
    pub scope: MemoryScope,
    pub survivor: MemoryId,
    /// The members it replaces, strongest first; never the survivor.
    pub members: Vec<MemoryId>,
    pub outcome: ClusterOutcomeKind,
    /// The merged support made a candidate survivor active.
    #[serde(default)]
    pub promoted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClusterOutcomeKind {
    /// A dry run: nothing was written.
    Planned,
    Merged,
    /// Something changed under the plan; the cluster was left as it was.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryExport {
    /// The JSON Lines artifact (§2.4).
    pub artifact: String,
    pub records: u32,
    /// SHA-256 of `artifact`, lowercase hex.
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryImport {
    pub dry_run: bool,
    pub digest: String,
    pub records: u32,
    pub inserted: u32,
    pub unchanged: u32,
    #[serde(default)]
    pub conflicts: Vec<ImportConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ImportConflict {
    pub memory: MemoryId,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryMigrationStatus {
    pub schema_version: u32,
    pub migrations: Vec<MemoryMigration>,
    pub memories: MemoryCounts,
    /// Newest first, by file name only.
    #[serde(default)]
    pub backups: Vec<MigrationBackup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_import: Option<LastImport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryMigration {
    pub version: u32,
    pub name: String,
    pub applied: bool,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct MemoryCounts {
    pub total: u32,
    pub admitted: u32,
    /// Memories with no admission row, read as active.
    pub implicit: u32,
    pub candidates: u32,
    pub stale: u32,
    /// Rows in the holdout log.
    pub held_out: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MigrationBackup {
    pub name: String,
    pub from_version: u32,
    pub to_version: u32,
    pub bytes: u64,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LastImport {
    pub at: Timestamp,
    pub digest: String,
    pub inserted: u32,
    pub unchanged: u32,
    pub conflicts: u32,
}

/// The export artifact's format name and the newest version this build reads.
pub const EXPORT_FORMAT: &str = "aether.memory-export";
pub const EXPORT_VERSION: u32 = 1;
/// Most records one export carries; a larger store is exported per scope.
pub const MAX_EXPORT_RECORDS: usize = 100_000;
/// Most bytes the artifact may take as an escaped JSON string: a 64 MiB frame
/// less room for the envelope, so the export reply and its import both fit.
pub const MAX_EXPORT_ENCODED_BYTES: usize = 48 * 1024 * 1024;

/// Line 1 of an export artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportHeader {
    pub format: String,
    pub version: u32,
    pub exported_at: Timestamp,
    pub schema: u32,
    pub scope: Option<MemoryScope>,
    pub records: u32,
}

/// Every line after the header: one memory as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportEntry {
    pub record: MemoryRecord,
    pub admission: Option<MemoryAdmission>,
    pub stale: Option<StaleNote>,
}
