// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Agent,
  AgentTree,
  ApprovalRequest,
  ApprovalRule,
  Artifact,
  Attention,
  AuthInfo,
  Checkpoint,
  DaemonStatus,
  Device,
  Event,
  ExportSummary,
  GitSummary,
  GithubWatcherStatus,
  ImportSummary,
  InboxItem,
  Invocation,
  InvocationSnapshot,
  McpServerHealth,
  MemoriesInContext,
  MemoryFeedback,
  MemoryFeedbackList,
  MemoryRecord,
  ModelInfo,
  Pairing,
  PathCompletions,
  ProviderInfo,
  PushConfig,
  RemoteStatus,
  ReplayFrame,
  RepositoryAlias,
  RepositoryAliasReport,
  RetentionPreview,
  RunListing,
  RunReport,
  Schedule,
  ScheduleOccurrence,
  SubmissionResolution,
  Task,
  TranscriptEntry,
  Welcome,
  Workspace,
  WorkspaceDiff,
  WorkspaceDiscard,
  WorkspaceIntegration,
  WorkspaceValidation,
  memories,
  pause,
  providers,
  replay,
  status,
} from "../../protocol.ts";

/**
 * Tagged by `reply`.
 */
export interface ReplyStorageReport {
  "data": unknown;
  "reply": "storage_report";
}

export interface ReplyRetentionPreview {
  "data": RetentionPreview;
  "reply": "retention_preview";
}

export interface ReplyGithubWatcher {
  "data": GithubWatcherStatus;
  "reply": "github_watcher";
}

export interface ReplySchedule {
  "data": Schedule;
  "reply": "schedule";
}

export interface ReplySchedules {
  "data": Array<Schedule>;
  "reply": "schedules";
}

export interface ReplyScheduleHistory {
  "data": Array<ScheduleOccurrence>;
  "reply": "schedule_history";
}

export interface ReplyWelcome {
  "data": Welcome;
  "reply": "welcome";
}

export interface ReplyStatus {
  "data": DaemonStatus;
  "reply": "status";
}

export interface ReplyOk {
  "reply": "ok";
}

/**
 * Reply to `Interrupt`: whether a turn was running to stop.
 */
export interface ReplyInterrupted {
  "data": Record<string, unknown>;
  "reply": "interrupted";
}

/**
 * Reply to `Pause`: the agents it moved, root first, empty when they
 * were already paused; `mid_turn` names those whose current turn must
 * finish before the pause takes effect.
 */
export interface ReplyPaused {
  "data": Record<string, unknown>;
  "reply": "paused";
}

export interface ReplyAgent {
  "data": Agent;
  "reply": "agent";
}

export interface ReplyAgents {
  "data": Array<Agent>;
  "reply": "agents";
}

export interface ReplyTree {
  "data": AgentTree;
  "reply": "tree";
}

export interface ReplyTask {
  "data": Task;
  "reply": "task";
}

export interface ReplyTasks {
  "data": Array<Task>;
  "reply": "tasks";
}

export interface ReplyMessageSent {
  "data": Record<string, unknown>;
  "reply": "message_sent";
}

/**
 * The note's event, by its sequence number in the log.
 */
export interface ReplyNoteAppended {
  "data": Record<string, unknown>;
  "reply": "note_appended";
}

/**
 * One entry per agent a fanned-out message reached, in delivery order.
 */
export interface ReplyMessagesSent {
  "data": Record<string, unknown>;
  "reply": "messages_sent";
}

export interface ReplyApprovals {
  "data": Array<ApprovalRequest>;
  "reply": "approvals";
}

export interface ReplyApprovalRule {
  "data": ApprovalRule;
  "reply": "approval_rule";
}

export interface ReplyApprovalRules {
  "data": Array<ApprovalRule>;
  "reply": "approval_rules";
}

export interface ReplySubscribed {
  "data": Record<string, unknown>;
  "reply": "subscribed";
}

export interface ReplyEvents {
  "data": Array<Event>;
  "reply": "events";
}

export interface ReplyTranscript {
  "data": Array<TranscriptEntry>;
  "reply": "transcript";
}

export interface ReplyInbox {
  "data": Array<InboxItem>;
  "reply": "inbox";
}

export interface ReplySnapshot {
  "data": InvocationSnapshot;
  "reply": "snapshot";
}

export interface ReplyInvocation {
  "data": Invocation;
  "reply": "invocation";
}

export interface ReplyInvocations {
  "data": Array<Invocation>;
  "reply": "invocations";
}

export interface ReplyCheckpoint {
  "data": Checkpoint | null;
  "reply": "checkpoint";
}

export interface ReplyArtifact {
  "data": Artifact;
  "reply": "artifact";
}

export interface ReplyArtifacts {
  "data": Array<Artifact>;
  "reply": "artifacts";
}

export interface ReplyArtifactBytes {
  "data": Record<string, unknown>;
  "reply": "artifact_bytes";
}

export interface ReplyMemories {
  "data": Array<MemoryRecord>;
  "reply": "memories";
}

export interface ReplyRepositoryAliased {
  "data": RepositoryAliasReport;
  "reply": "repository_aliased";
}

export interface ReplyRepositoryAliases {
  "data": Array<RepositoryAlias>;
  "reply": "repository_aliases";
}

export interface ReplyMemoriesInContext {
  "data": MemoriesInContext;
  "reply": "memories_in_context";
}

export interface ReplyMemory {
  "data": MemoryRecord;
  "reply": "memory";
}

export interface ReplyMemoryFeedback {
  "data": MemoryFeedback;
  "reply": "memory_feedback";
}

export interface ReplyMemoryFeedbackList {
  "data": MemoryFeedbackList;
  "reply": "memory_feedback_list";
}

export interface ReplyMemorySeeded {
  "data": Record<string, unknown>;
  "reply": "memory_seeded";
}

export interface ReplyRuns {
  "data": Array<RunListing>;
  "reply": "runs";
}

export interface ReplyRunReport {
  "data": RunReport;
  "reply": "run_report";
}

export interface ReplyReplay {
  "data": Array<ReplayFrame>;
  "reply": "replay";
}

export interface ReplyRunExported {
  "data": ExportSummary;
  "reply": "run_exported";
}

export interface ReplyRunImported {
  "data": ImportSummary;
  "reply": "run_imported";
}

export interface ReplyMcpServers {
  "data": Array<McpServerHealth>;
  "reply": "mcp_servers";
}

export interface ReplyModels {
  "data": Array<ModelInfo>;
  "reply": "models";
}

export interface ReplyProviders {
  "data": Array<ProviderInfo>;
  "reply": "providers";
}

export interface ReplyProviderUsage {
  "data": unknown;
  "reply": "provider_usage";
}

export interface ReplyAuth {
  "data": Array<AuthInfo>;
  "reply": "auth";
}

export interface ReplyEngineTool {
  "data": Record<string, unknown>;
  "reply": "engine_tool";
}

export interface ReplyEngineTools {
  "data": Record<string, unknown>;
  "reply": "engine_tools";
}

export interface ReplyAttention {
  "data": Attention;
  "reply": "attention";
}

export interface ReplyPathCompletions {
  "data": PathCompletions;
  "reply": "path_completions";
}

export interface ReplyWorkspaces {
  "data": Array<Workspace>;
  "reply": "workspaces";
}

export interface ReplyWorkspaceValidation {
  "data": WorkspaceValidation;
  "reply": "workspace_validation";
}

export interface ReplyGitSummary {
  "data": GitSummary;
  "reply": "git_summary";
}

export interface ReplyWorkspaceDiff {
  "data": WorkspaceDiff;
  "reply": "workspace_diff";
}

export interface ReplyWorkspaceIntegration {
  "data": WorkspaceIntegration;
  "reply": "workspace_integration";
}

export interface ReplyWorkspaceDiscarded {
  "data": WorkspaceDiscard;
  "reply": "workspace_discarded";
}

export interface ReplyRemote {
  "data": RemoteStatus;
  "reply": "remote";
}

export interface ReplyPairing {
  "data": Pairing;
  "reply": "pairing";
}

export interface ReplyDevice {
  "data": Device;
  "reply": "device";
}

export interface ReplyDevices {
  "data": Array<Device>;
  "reply": "devices";
}

export interface ReplyPushConfig {
  "data": PushConfig;
  "reply": "push_config";
}

export interface ReplySubmissionResolved {
  "data": SubmissionResolution;
  "reply": "submission_resolved";
}

export type Reply =
    ReplyStorageReport |
    ReplyRetentionPreview |
    ReplyGithubWatcher |
    ReplySchedule |
    ReplySchedules |
    ReplyScheduleHistory |
    ReplyWelcome |
    ReplyStatus |
    ReplyOk |
    ReplyInterrupted |
    ReplyPaused |
    ReplyAgent |
    ReplyAgents |
    ReplyTree |
    ReplyTask |
    ReplyTasks |
    ReplyMessageSent |
    ReplyNoteAppended |
    ReplyMessagesSent |
    ReplyApprovals |
    ReplyApprovalRule |
    ReplyApprovalRules |
    ReplySubscribed |
    ReplyEvents |
    ReplyTranscript |
    ReplyInbox |
    ReplySnapshot |
    ReplyInvocation |
    ReplyInvocations |
    ReplyCheckpoint |
    ReplyArtifact |
    ReplyArtifacts |
    ReplyArtifactBytes |
    ReplyMemories |
    ReplyRepositoryAliased |
    ReplyRepositoryAliases |
    ReplyMemoriesInContext |
    ReplyMemory |
    ReplyMemoryFeedback |
    ReplyMemoryFeedbackList |
    ReplyMemorySeeded |
    ReplyRuns |
    ReplyRunReport |
    ReplyReplay |
    ReplyRunExported |
    ReplyRunImported |
    ReplyMcpServers |
    ReplyModels |
    ReplyProviders |
    ReplyProviderUsage |
    ReplyAuth |
    ReplyEngineTool |
    ReplyEngineTools |
    ReplyAttention |
    ReplyPathCompletions |
    ReplyWorkspaces |
    ReplyWorkspaceValidation |
    ReplyGitSummary |
    ReplyWorkspaceDiff |
    ReplyWorkspaceIntegration |
    ReplyWorkspaceDiscarded |
    ReplyRemote |
    ReplyPairing |
    ReplyDevice |
    ReplyDevices |
    ReplyPushConfig |
    ReplySubmissionResolved
;
