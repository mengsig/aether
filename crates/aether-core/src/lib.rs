//! Aether core: the domain vocabulary shared by every crate.
//!
//! Pure data and pure logic only. No I/O, no async runtime, no provider.
//! State machines live here so every store, runtime and UI agrees on which
//! transitions are legal.

pub mod agent;
pub mod approval;
pub mod artifact;
pub mod attachment;
pub mod budget;
pub mod checkpoint;
pub mod disk;
pub mod error;
pub mod event;
pub mod github;
pub mod id;
pub mod idempotency;
pub mod inbox;
pub mod lease;
pub mod mcp;
pub mod memory;
pub mod model;
pub mod private_file;
pub mod provider;
pub mod redact;
pub mod replay;
pub mod report;
pub mod repository;
pub mod retention;
pub mod schedule;
pub mod snapshot;
pub mod task;
pub mod text;
pub mod time;
pub mod tool;
pub mod transfer;

/// The commonly used names, for `use aether_core::prelude::*`.
pub mod prelude {
    pub use crate::agent::{
        Agent, AgentConfig, AgentStats, AgentStatus, AgentStatusKind, ApprovalPolicy,
        ExecutionBackend, NetworkPolicy, Role, Scope, WaitCondition, WaitOracle,
    };
    pub use crate::approval::{
        ApprovalRule, CallSubject, Revocation, RuleContext, RuleMatch, RuleScope,
    };
    pub use crate::artifact::{Artifact, ArtifactKind};
    pub use crate::attachment::{AttachmentId, AttachmentRef, ImageAttachment};
    pub use crate::budget::{RunSummary, active_runs, subtree_usage};
    pub use crate::checkpoint::{
        Checkpoint, CheckpointDraft, CheckpointError, CheckpointProvenance,
    };
    pub use crate::error::TransitionError;
    pub use crate::event::{Actor, CompactionTrigger, Event, EventDraft, EventKind, RecoveryClass};
    pub use crate::id::*;
    pub use crate::idempotency::{IdempotencyKey, RecordedOutcome, RetryPolicy};
    pub use crate::inbox::{InboxItem, InboxKind};
    pub use crate::lease::Lease;
    pub use crate::mcp::{McpServerHealth, McpServerState, McpToolInfo};
    pub use crate::memory::{
        MemoryCandidate, MemoryKind, MemoryProvenance, MemoryRecord, MemoryScope, MemorySource,
        SeedReport,
    };
    pub use crate::model::{
        BlockStart, CapabilityProfile, ContentBlock, Effort, Message, MessageRole, ModelCost,
        ModelError, ModelErrorClass, ModelRef, ModelRequest, ModelResponse, StopReason,
        StreamAccumulator, StreamEvent, SystemBlock, ThinkingMode, ThinkingSupport, ToolChoice,
        ToolResultBlock, ToolSpec, Usage,
    };
    pub use crate::provider::{
        ErrorTally, ErrorWindow, FallbackRecord, ProviderBackoff, ProviderHealth, QuotaWindow,
    };
    pub use crate::replay::{ReplayFrame, TurnOutcome};
    pub use crate::report::{RunBody, RunOutcome, RunReport};
    pub use crate::snapshot::{
        ContextLayer, Invocation, InvocationParams, InvocationSnapshot, LayerKind, LayerPlacement,
    };
    pub use crate::task::{Priority, Task, TaskResult, TaskStatus, TaskStatusKind};
    pub use crate::time::{Timestamp, now};
    pub use crate::tool::{
        ApprovalDecision, ApprovalRequest, ApprovalStatus, ResourceKey, SideEffectClass, ToolCall,
        ToolDescriptor, ToolExposure, ToolOutcome,
    };
}
