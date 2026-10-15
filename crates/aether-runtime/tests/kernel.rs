#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Deterministic kernel tests: the brief's list, run against a scripted
//! model so no network and no real provider are involved.
use std::sync::Arc;

use std::time::Duration;

use aether_core::agent::{
    AgentConfig, AgentStatus, AgentStatusKind, ApprovalPolicy, ArchiveTarget, BlockReason,
    DeadlineUpdate, Scope,
};

use aether_core::approval::{RuleMatch, RuleScope};

use aether_core::artifact::RetentionPolicy;

use aether_core::event::{Actor, CompactionTrigger, EventDraft, EventKind};

use aether_core::id::{AgentId, ToolCallId};

use aether_core::idempotency::RecordedOutcome;

use aether_core::lease::Lease;

use aether_core::model::{BlockStart, ContentBlock, ModelErrorClass, ModelRef, StreamEvent};

use aether_core::snapshot::LayerKind;

use aether_core::task::TaskStatusKind;

use aether_core::time::{Timestamp, now};

use aether_core::tool::{ApprovalDecision, SideEffectClass, ToolCall, ToolOutcome};

use aether_model::Models;

use aether_model::testing::ScriptedClient;

use aether_runtime::{AgentUpdate, CreateAgentSpec, Remember, Runtime, RuntimeConfig};

use aether_store::{
    AgentFilter, ArtifactFilter, Commit, EventFilter, Mutation, SqliteStore, Store,
};

use aether_tools::ToolRegistry;

use futures::FutureExt;

use serde_json::json;

fn types(store: &dyn Store, id: AgentId) -> Vec<String> {
    store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events")
        .iter()
        .map(|e| e.type_name())
        .collect()
}

fn spec(objective: &str) -> CreateAgentSpec {
    CreateAgentSpec {
        role: "worker".into(),
        objective: Some(objective.into()),
        start: true,
        config: Some(AgentConfig {
            effort: Some(aether_core::model::Effort::Low),
            approval: ApprovalPolicy::none(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

use store_double::{CommitsMidPass, Hook};

use aether_core::agent::Agent;

use aether_core::approval::ApprovalRule;

use aether_core::artifact::Artifact;

use aether_core::checkpoint::Checkpoint;

use aether_core::event::Event;

use aether_core::id::{
    ApprovalId, ApprovalRuleId, ArtifactId, CheckpointId, EventId, InvocationId, MemoryId,
    SnapshotId, TaskId,
};

use aether_core::idempotency::IdempotencyKey;

use aether_core::inbox::InboxItem;

use aether_core::memory::MemoryRecord;

use aether_core::snapshot::{Invocation, InvocationSnapshot};

use aether_core::task::Task;

use aether_core::tool::ApprovalRequest;

use aether_store::Result as StoreResult;

use aether_store::{MemoryQuery, TaskFilter, TranscriptEntry};

use std::path::PathBuf;

#[path = "kernel/agent_support.rs"]
mod agent_support;
#[path = "kernel/approval_dispatch.rs"]
mod approval_dispatch;
#[path = "kernel/approval_policy.rs"]
mod approval_policy;
#[path = "kernel/approval_rules.rs"]
mod approval_rules;
#[path = "kernel/archive_checkpoint.rs"]
mod archive_checkpoint;
#[path = "kernel/artifact_dedup.rs"]
mod artifact_dedup;
#[path = "kernel/assignment_delivery.rs"]
mod assignment_delivery;
#[path = "kernel/attachments.rs"]
mod attachments;
#[path = "kernel/background_tools.rs"]
mod background_tools;
#[path = "kernel/budget_delegation.rs"]
mod budget_delegation;
#[path = "kernel/budgets.rs"]
mod budgets;
#[path = "kernel/cancellation_races.rs"]
mod cancellation_races;
#[path = "kernel/cancellation_restart.rs"]
mod cancellation_restart;
#[path = "kernel/chat_retries.rs"]
mod chat_retries;
#[path = "kernel/checkpoints.rs"]
mod checkpoints;
#[path = "kernel/commit_races.rs"]
mod commit_races;
#[path = "kernel/completion_artifacts.rs"]
mod completion_artifacts;
#[path = "kernel/crash_cancellation.rs"]
mod crash_cancellation;
#[path = "kernel/crash_recovery.rs"]
mod crash_recovery;
#[path = "kernel/delegation.rs"]
mod delegation;
#[path = "kernel/dispatch_priority.rs"]
mod dispatch_priority;
#[path = "kernel/engine_fallback_slots.rs"]
mod engine_fallback_slots;
#[path = "kernel/engine_handoffs.rs"]
mod engine_handoffs;
#[path = "kernel/engine_slot_recovery.rs"]
mod engine_slot_recovery;
#[path = "kernel/engine_slots.rs"]
mod engine_slots;
#[path = "kernel/execution_policy.rs"]
mod execution_policy;
#[path = "kernel/exhaustion_classification.rs"]
mod exhaustion_classification;
#[path = "kernel/exhaustion_support.rs"]
mod exhaustion_support;
#[path = "kernel/fallback_accounting.rs"]
mod fallback_accounting;
#[path = "kernel/fallback_guards.rs"]
mod fallback_guards;
#[path = "kernel/fallback_transition.rs"]
mod fallback_transition;
#[path = "kernel/fixture.rs"]
mod fixture;
#[path = "kernel/held_agents.rs"]
mod held_agents;
#[path = "kernel/held_waits.rs"]
mod held_waits;
#[path = "kernel/lease_reads.rs"]
mod lease_reads;
#[path = "kernel/leases.rs"]
mod leases;
#[path = "kernel/memory_scope.rs"]
mod memory_scope;
#[path = "kernel/memory_support.rs"]
mod memory_support;
#[path = "kernel/message_fanout.rs"]
mod message_fanout;
#[path = "kernel/model_effort.rs"]
mod model_effort;
#[path = "kernel/model_profiles.rs"]
mod model_profiles;
#[path = "kernel/operator_memory.rs"]
mod operator_memory;
#[path = "kernel/operator_rename.rs"]
mod operator_rename;
#[path = "kernel/parent_waits.rs"]
mod parent_waits;
#[path = "kernel/pause_recovery.rs"]
mod pause_recovery;
#[path = "kernel/prompt_integrity.rs"]
mod prompt_integrity;
#[path = "kernel/provider_probes.rs"]
mod provider_probes;
#[path = "kernel/provider_scheduling.rs"]
mod provider_scheduling;
#[path = "kernel/question_guards.rs"]
mod question_guards;
#[path = "kernel/questions.rs"]
mod questions;
#[path = "kernel/recovery_limits.rs"]
mod recovery_limits;
#[path = "kernel/recovery_support.rs"]
mod recovery_support;
#[path = "kernel/repository_alias.rs"]
mod repository_alias;
#[path = "kernel/repository_alias_races.rs"]
mod repository_alias_races;
#[path = "kernel/repository_memory.rs"]
mod repository_memory;
#[path = "kernel/repository_root.rs"]
mod repository_root;
#[path = "kernel/restart_limits.rs"]
mod restart_limits;
#[path = "kernel/restart_scope.rs"]
mod restart_scope;
#[path = "kernel/restart_wait.rs"]
mod restart_wait;
#[path = "kernel/restart_wake.rs"]
mod restart_wake;
#[path = "kernel/scheduled_state.rs"]
mod scheduled_state;
#[path = "kernel/scheduling_support.rs"]
mod scheduling_support;
#[path = "kernel/shell_leases.rs"]
mod shell_leases;
#[path = "kernel/shutdown_recovery.rs"]
mod shutdown_recovery;
#[path = "kernel/snapshot_reconstruction.rs"]
mod snapshot_reconstruction;
#[path = "kernel/store_double.rs"]
mod store_double;
#[path = "kernel/store_outage.rs"]
mod store_outage;
#[path = "kernel/subtree_cancellation.rs"]
mod subtree_cancellation;
#[path = "kernel/subtree_policy.rs"]
mod subtree_policy;
#[path = "kernel/supervisor_dispatch.rs"]
mod supervisor_dispatch;
#[path = "kernel/suspension.rs"]
mod suspension;
#[path = "kernel/suspension_control.rs"]
mod suspension_control;
#[path = "kernel/task_dependencies.rs"]
mod task_dependencies;
#[path = "kernel/task_lifecycle.rs"]
mod task_lifecycle;
#[path = "kernel/tool_idempotency.rs"]
mod tool_idempotency;
#[path = "kernel/turn_robustness.rs"]
mod turn_robustness;
#[path = "kernel/unanswered_turns.rs"]
mod unanswered_turns;
#[path = "kernel/wait_conditions.rs"]
mod wait_conditions;
#[path = "kernel/waiting.rs"]
mod waiting;
#[path = "kernel/workspace_isolation.rs"]
mod workspace_isolation;
use agent_support::{
    CallOutside, child_of, idle_spec, long_lived, paused, settled_outcome, waits_on_inbox,
};
use exhaustion_support::{
    NO_PROBE_IN_THIS_TEST, bench_history, blocked_on_exhaustion, child_failures, child_suspensions,
    exhausted_reason, exhaustion_records, parent_and_suspending_child, schedule_of,
};
use fixture::{
    Harness, declared_engines, harness, harness_configured, harness_full,
    harness_full_with_engines, harness_with, harness_with_tools, spawns_real_bash,
};
use memory_support::{git_in, wait_for_seeding};
use recovery_support::{
    join_racer, race_through_runtime, recovery_class, restart_over, second_daemon_over,
    seed_crashed_turn, seed_interrupted, wait_for_event, wait_until_events,
};
use scheduling_support::{
    assert_never_overlapped, first_event_seq, shell_window, slow_turn, spec_on, turn_intervals,
    turn_of_length, wait_for_tool_answer,
};
use waiting::{
    WAIT_UNTIL_BASH_HARD_CEILING, WAIT_UNTIL_FAST_HARD_CEILING, WAIT_UNTIL_STALL_WINDOW,
    activity_signature, system_load_snapshot, timeout_details, wait_for_marker,
    wait_for_with_budget, wait_until_with_budget,
};
