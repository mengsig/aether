// The Aether daemon protocol as TypeScript types and command builders.
// Generated from clients/protocol.schema.json by web/scripts/generate-protocol.ts.
// Do not edit: regenerate with `npm run generate` in web/.
// Schema SHA-256: 70f2e1a101f057452b5231080f7e3ff36eb9f8fdfe9268a66f6450c60c4ade1d

export const PROTOCOL_VERSION = 26;
export const MIN_PROTOCOL_VERSION = 17;

/**
 * Who caused an event.
 * Tagged by `actor`.
 */
export interface ActorSystem {
  "actor": "system";
}

export interface ActorScheduler {
  "actor": "scheduler";
}

export interface ActorUser {
  "actor": "user";
  "name"?: string | null;
  "origin"?: OperatorOrigin | null;
}

export interface ActorAgent {
  "actor": "agent";
  "id": AgentId;
}

export interface ActorTool {
  "actor": "tool";
  "call": ToolCallId;
}

/**
 * A remembered decision answered on a person's behalf.
 */
export interface ActorRule {
  "actor": "rule";
  "rule": ApprovalRuleId;
}

export interface ActorClient {
  "actor": "client";
  "id": string;
}

/**
 * An actor that acted on another machine, read out of an export. The
 * original travels for the record; nothing here acted.
 */
export interface ActorImported {
  "actor": "imported";
  "was": Actor;
}

export type Actor =
  | ActorSystem
  | ActorScheduler
  | ActorUser
  | ActorAgent
  | ActorTool
  | ActorRule
  | ActorClient
  | ActorImported;

/**
 * A durable agent record. This is authoritative state; the model is never
 * asked to remember any of it.
 */
export interface Agent {
  "archived"?: boolean;
  "checkpoint"?: CheckpointId | null;
  "config": AgentConfig;
  "created_at": string;
  "current_task"?: TaskId | null;
  "engine_alive"?: boolean;
  "engine_sent"?: EngineSentLayers | null;
  "engine_session"?: string | null;
  "engine_usage"?: EngineSessionUsage | null;
  "fallbacks"?: Array<ModelRef>;
  "id": AgentId;
  "inbox_cursor"?: number;
  "last_event_at"?: string | null;
  "model": ModelRef;
  "name": string;
  "parent"?: AgentId | null;
  "paused"?: boolean;
  "paused_via"?: AgentId | null;
  "restarted_as"?: AgentId | null;
  "restarted_from"?: AgentId | null;
  "resume_wait"?: WaitCondition | null;
  "role": Role;
  "run": RunId;
  "scope": Scope;
  "stats"?: AgentStats;
  "status": AgentStatus;
  "status_since"?: string | null;
  "updated_at": string;
}

/**
 * A failed agent, or one blocked on something only the operator can clear.
 */
export interface AgentAttention {
  "agent": AgentId;
  "agent_name"?: string | null;
  "kind": AttentionKind;
  "reason": string;
  "since": string;
}

/**
 * Per-agent configuration the runtime enforces.
 */
export interface AgentConfig {
  "approval"?: ApprovalPolicy;
  "build_cache"?: string | null;
  "concurrency"?: number | null;
  "cost_budget"?: number | null;
  "deadline"?: string | null;
  "effort"?: Effort | null;
  "host_context"?: HostContext;
  "instructions"?: string | null;
  "interactive"?: boolean | null;
  "leaf"?: boolean;
  "long_lived"?: boolean;
  "max_output_tokens"?: number | null;
  "max_turns"?: number | null;
  "priority"?: Priority;
  "token_budget"?: number | null;
  "tools"?: ToolExposure;
}

/**
 * A agt_ prefixed ULID.
 * Pattern: ^agt_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type AgentId = string;

export interface AgentLine {
  "created_at": string;
  "depth": number;
  "ended_at"?: string | null;
  "id": AgentId;
  "model": ModelRef;
  "name": string;
  "outcome": Ended;
  "parent"?: AgentId | null;
  "role": Role;
  "subtree_usage": Usage;
  "tool_calls": number;
  "turns": number;
  "usage": Usage;
  "workspace"?: string | null;
}

/**
 * Partial update to a live agent. Only set fields change.
 */
export interface AgentPatch {
  "approval"?: ApprovalPolicy | null;
  "backend"?: ExecutionBackend | null;
  "concurrency"?: number | null;
  "cost_budget"?: number | null;
  "deadline"?: DeadlineUpdate | null;
  "effort"?: Effort | null;
  "fallbacks"?: Array<ModelRef> | null;
  "instructions"?: string | null;
  "long_lived"?: boolean | null;
  "max_turns"?: number | null;
  "model"?: ModelRef | null;
  "name"?: string | null;
  "network"?: NetworkPolicy | null;
  "priority"?: Priority | null;
  "token_budget"?: number | null;
  "tools"?: ToolExposure | null;
}

export interface AgentQuery {
  "include_archived"?: boolean;
  "live_only"?: boolean;
  "parent"?: AgentId | null;
  "run"?: RunId | null;
  "statuses"?: Array<AgentStatusKind>;
}

/**
 * Lifetime token counters and activity stats.
 */
export interface AgentStats {
  "context_tokens"?: number | null;
  "last_turn_at"?: string | null;
  "tool_calls"?: number;
  "turns"?: number;
  "usage"?: Usage;
}

/**
 * Tagged by `state`.
 */
export interface AgentStatusCreated {
  "state": "created";
}

export interface AgentStatusReady {
  "state": "ready";
}

export interface AgentStatusRunning {
  "state": "running";
}

export interface AgentStatusWaiting {
  "condition": WaitCondition;
  "state": "waiting";
}

export interface AgentStatusBlocked {
  "reason": BlockReason;
  "state": "blocked";
}

export interface AgentStatusCompleted {
  "state": "completed";
  "summary"?: string | null;
}

export interface AgentStatusFailed {
  "error": string;
  "state": "failed";
}

export interface AgentStatusCancelled {
  "reason"?: string | null;
  "state": "cancelled";
}

/**
 * Read out of another daemon's export. Terminal by construction: an
 * imported agent is a record of work that happened elsewhere, and no
 * local command or scheduler pass may give it a turn.
 */
export interface AgentStatusImported {
  "state": "imported";
  "was": AgentStatusKind;
}

export type AgentStatus =
  | AgentStatusCreated
  | AgentStatusReady
  | AgentStatusRunning
  | AgentStatusWaiting
  | AgentStatusBlocked
  | AgentStatusCompleted
  | AgentStatusFailed
  | AgentStatusCancelled
  | AgentStatusImported;

/**
 * The variant of an [`AgentStatus`] without its payload.
 */
export type AgentStatusKind = "created" | "ready" | "running" | "waiting" | "blocked" | "completed" | "failed" | "cancelled" | "imported";

export interface AgentTree {
  "nodes": Record<string, unknown>;
  "roots": Array<AgentId>;
}

/**
 * A learner's answer to an open question.
 */
export interface AnswerInput {
  "choices"?: Array<string> | null;
  "confidence"?: number | null;
  "dont_know": boolean;
  "text"?: string | null;
  "thinking"?: string | null;
}

/**
 * What an answer revealed: one value for a recall question, several for a
 * multiple choice one.
 */
export type AnswerKey = Array<string> | string;

export type ApprovalDecision = "approved" | "denied";

/**
 * What approval policy an agent gets when nothing else decides one. The
 * shipped default is `Yolo`: the operator asked for auto-approve out of the
 * box, and the execution backend - not the prompt - is the isolation
 * boundary.
 */
export type ApprovalDefault = "yolo" | "ask";

/**
 * A apr_ prefixed ULID.
 * Pattern: ^apr_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type ApprovalId = string;

export interface ApprovalLine {
  "decided_at"?: string | null;
  "decided_by"?: Actor | null;
  "decision"?: ApprovalDecision | null;
  "description": string;
  "id": ApprovalId;
  "requested_at": string;
  "tool": string;
}

/**
 * Which side-effect classes need explicit approval before execution.
 */
export interface ApprovalPolicy {
  "require_for"?: Array<SideEffectClass>;
}

/**
 * A request for a human (or supervising agent) to allow a tool call.
 */
export interface ApprovalRequest {
  "agent_id": AgentId;
  "argument"?: string | null;
  "call_id": ToolCallId;
  "description": string;
  "id": ApprovalId;
  "requested_at": string;
  "resources"?: Array<ResourceKey>;
  "side_effect": SideEffectClass;
  "status": ApprovalStatus;
  "task_id"?: TaskId | null;
  "tool": string;
}

/**
 * A decision a person asked Aether to remember.
 */
export interface ApprovalRule {
  "created_at": string;
  "created_by": Actor;
  "decision": ApprovalDecision;
  "id": ApprovalRuleId;
  "match": RuleMatch;
  "note"?: string | null;
  "revoked"?: Revocation | null;
  "scope": RuleScope;
}

/**
 * A rule_ prefixed ULID.
 * Pattern: ^rule_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type ApprovalRuleId = string;

/**
 * Tagged by `state`.
 */
export interface ApprovalStatusPending {
  "state": "pending";
}

export interface ApprovalStatusResolved {
  "at": string;
  "by": Actor;
  "decision": ApprovalDecision;
  "note"?: string | null;
  "state": "resolved";
}

export type ApprovalStatus = ApprovalStatusPending | ApprovalStatusResolved;

/**
 * What an archive command marks: one finished agent, every finished agent of
 * a run, or every finished agent there is.
 */
export type ArchiveTarget = "all_finished" | Record<string, unknown>;

export interface Artifact {
  "agent_id"?: AgentId | null;
  "content_type"?: string | null;
  "created_at": string;
  "created_by": Actor;
  "digest"?: string | null;
  "evicted_at"?: string | null;
  "id": ArtifactId;
  "kind": ArtifactKind;
  "name": string;
  "path"?: string | null;
  "size_bytes": number;
  "summary": string;
  "task_id"?: TaskId | null;
}

/**
 * A art_ prefixed ULID.
 * Pattern: ^art_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type ArtifactId = string;

/**
 * Tagged by `kind`.
 */
export interface ArtifactKindFile {
  "kind": "file";
}

export interface ArtifactKindCommandOutput {
  "kind": "command_output";
}

export interface ArtifactKindDiff {
  "kind": "diff";
}

export interface ArtifactKindCommit {
  "kind": "commit";
}

export interface ArtifactKindResult {
  "kind": "result";
}

export interface ArtifactKindOther {
  "kind": "other";
  "label": string;
}

export type ArtifactKind =
  | ArtifactKindFile
  | ArtifactKindCommandOutput
  | ArtifactKindDiff
  | ArtifactKindCommit
  | ArtifactKindResult
  | ArtifactKindOther;

export interface ArtifactLine {
  "agent"?: AgentId | null;
  "created_at": string;
  "evicted"?: boolean;
  "id": ArtifactId;
  "kind": string;
  "name": string;
  "size_bytes": number;
  "summary": string;
  "task"?: TaskId | null;
}

/**
 * A reference to an important artifact, kept as a note rather than a copy.
 */
export interface ArtifactNote {
  "note": string;
  "reference"?: string | null;
}

export interface ArtifactQuery {
  "agent"?: AgentId | null;
  "limit"?: number;
  "name"?: string | null;
  "task"?: TaskId | null;
  "with_bytes_only"?: boolean;
}

/**
 * The SHA-256 of the stored bytes, in lowercase hex.
 * Pattern: ^[0-9a-f]{64}$
 */
export type AttachmentId = string;

/**
 * A stored image, as inboxes, events and transcripts carry it.
 */
export interface AttachmentRef {
  "id": AttachmentId;
  "media_type": string;
  "name"?: string | null;
  "size_bytes": number;
}

/**
 * Everything waiting on the operator, read at one sequence number.
 */
export interface Attention {
  "agents": Array<AgentAttention>;
  "approvals": Array<ApprovalRequest>;
  "last_seq": number;
  "questions": Array<PendingQuestion>;
}

/**
 * What can wait on the operator, and what a push notification is about.
 */
export type AttentionKind = "approval" | "question" | "agent_failed" | "agent_blocked" | "task_completed" | "task_failed" | "test";

export interface AuthInfo {
  "configured": boolean;
  "detail"?: string | null;
  "methods"?: Array<string>;
  "program"?: string | null;
  "provider": string;
  "provider_name": string;
  "source"?: string | null;
  "subscription"?: boolean;
}

export type BackgroundDisable = "cancel_at_model_boundary";

export type BackgroundPriority = "low" | "normal";

export interface BillingPatch {
  "allow_api"?: boolean | null;
}

export interface BillingSettings {
  "allow_api": boolean;
}

/**
 * Why an agent is Blocked. One variant per cause, each carrying what that
 * cause needs, so a Blocked agent is never ambiguous about what would clear
 * it. An operator pause is not here: it is orthogonal to scheduling state
 * and lives in [`Agent::paused`] (VISION §3.1).
 * Tagged by `block`.
 */
/**
 * VISION §3.2: the owner of a task whose dependencies are not terminal.
 * Cleared only when that dependency finishes.
 */
export interface BlockReasonDependency {
  "block": "dependency";
  "on": TaskId;
}

/**
 * VISION §3.6 limit 5: a live turn waiting for a resource key another
 * agent holds. The turn itself resumes when the key comes free.
 */
export interface BlockReasonResource {
  "block": "resource";
  "holder": AgentId;
  "key": ResourceKey;
}

/**
 * VISION §3.9: a non-idempotent tool call started without an outcome.
 * Only a person clears it.
 */
export interface BlockReasonInspection {
  "block": "inspection";
  "detail": string;
}

/**
 * Provider credits/allowance spent on every link of the chain. Cleared by
 * the scheduler when `next_probe` comes due, or by a person.
 */
export interface BlockReasonExhausted {
  "block": "exhausted";
  "class": ExhaustionClass;
  "detail": string;
  "model": ModelRef;
  "next_probe": string;
  "probes": number;
  "reported_reset"?: string | null;
  "since": string;
}

export type BlockReason =
  | BlockReasonDependency
  | BlockReasonResource
  | BlockReasonInspection
  | BlockReasonExhausted;

export interface BudgetPatch {
  "background_tokens_daily"?: number | null;
  "background_usd_daily"?: number | null;
  "interactive_usd_daily"?: number | null;
}

export interface BudgetSettings {
  "background_tokens_daily": number;
  "background_usd_daily": number;
  "interactive_usd_daily": number;
}

/**
 * What an occurrence does when the target is busy. Either way at most one
 * trigger from a schedule is ever pending in the target's inbox.
 */
export type BusyPolicy = "skip" | "queue_one";

/**
 * What a model can do. Declared per model in the registry so callers can ask
 * before they depend on a feature.
 */
export interface CapabilityProfile {
  "compaction_tokens"?: number | null;
  "context_window": number;
  "cost"?: ModelCost | null;
  "effort"?: boolean;
  "extensions"?: Record<string, unknown>;
  "max_output_tokens": number;
  "parallel_tools"?: boolean;
  "prompt_caching"?: boolean;
  "strict_tools"?: boolean;
  "structured_output"?: boolean;
  "thinking"?: ThinkingSupport;
  "tools"?: boolean;
  "vision"?: boolean;
}

export interface Chain {
  "conclusion": string;
  "created_at": string;
  "id": string;
  "premises": Array<string>;
  "provenance": Array<NoesisProvenance>;
}

export interface ChangedFile {
  "deletions"?: number | null;
  "insertions"?: number | null;
  "path": string;
  "status": FileStatus;
}

/**
 * A durable, validated checkpoint.
 */
export interface Checkpoint {
  "agent_id": AgentId;
  "created_at": string;
  "draft": CheckpointDraft;
  "id": CheckpointId;
  "provenance": CheckpointProvenance;
  "version": number;
}

/**
 * The model-facing checkpoint schema. This is what a compactor model is
 * asked to produce; the runtime wraps it into a [`Checkpoint`].
 */
export interface CheckpointDraft {
  "artifacts"?: Array<ArtifactNote>;
  "constraints"?: Array<string>;
  "decisions"?: Array<string>;
  "dropped_constraints"?: Array<DroppedItem>;
  "objective": string;
  "open_loops"?: Array<string>;
  "resolved_open_loops"?: Array<string>;
  "state": string;
  "uncertainty"?: Array<string>;
}

/**
 * A chk_ prefixed ULID.
 * Pattern: ^chk_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type CheckpointId = string;

/**
 * Where a checkpoint came from.
 */
export interface CheckpointProvenance {
  "compactor_model"?: ModelRef | null;
  "from_seq": number;
  "previous"?: CheckpointId | null;
  "source_snapshot"?: SnapshotId | null;
  "to_seq": number;
  "trigger"?: CompactionTrigger | null;
}

/**
 * Client to daemon. The request id is `req` on the wire so it can never
 * collide with a command's own `id` field after flattening.
 */
export type ClientFrame = { "req": number } & Command;

export interface Cluster {
  "id": string;
  "members": Array<string>;
  "session": string;
}

export type ClusterOutcomeKind = "merged" | "planned" | "skipped";

/**
 * What triggered a compaction.
 * Tagged by `trigger`.
 */
export interface CompactionTriggerTokenThreshold {
  "threshold": number;
  "tokens": number;
  "trigger": "token_threshold";
}

export interface CompactionTriggerTaskBoundary {
  "task": TaskId;
  "trigger": "task_boundary";
}

export interface CompactionTriggerLargeToolOutput {
  "call": ToolCallId;
  "trigger": "large_tool_output";
}

export interface CompactionTriggerLongWait {
  "trigger": "long_wait";
}

export interface CompactionTriggerExplicit {
  "by": Actor;
  "trigger": "explicit";
}

/**
 * The agent's model moved across the engine/API boundary. `from` is the
 * model that produced the context being folded, so it can compact it.
 */
export interface CompactionTriggerHandoff {
  "from": ModelRef;
  "session"?: string | null;
  "to": ModelRef;
  "trigger": "handoff";
}

export type CompactionTrigger =
  | CompactionTriggerTokenThreshold
  | CompactionTriggerTaskBoundary
  | CompactionTriggerLargeToolOutput
  | CompactionTriggerLongWait
  | CompactionTriggerExplicit
  | CompactionTriggerHandoff;

export type ConceptKind = "proposition" | "plan_node" | "topic";

export interface ConceptRef {
  "id": string;
  "kind": ConceptKind;
  "ref": string;
  "title": string;
}

export interface ConceptState {
  "as_of_evidence": number;
  "calibration"?: number | null;
  "concept": ConceptRef;
  "last_success_at"?: string | null;
  "misconceptions": Array<string>;
  "review_due_at"?: string | null;
  "state": ConceptStateKind;
  "strength": number;
}

export type ConceptStateKind = "unseen" | "exposed" | "demonstrated" | "fragile" | "misconception";

export interface ConcurrencyPatch {
  "background"?: number | null;
  "interactive"?: number | null;
}

export interface ConcurrencySettings {
  "background": number;
  "interactive": number;
}

export interface ConsolidatedCluster {
  "members": Array<MemoryId>;
  "outcome": ClusterOutcomeKind;
  "promoted"?: boolean;
  "reason"?: string | null;
  "scope": MemoryScope;
  "survivor": MemoryId;
}

/**
 * A block inside a message.
 * Tagged by `type`.
 */
export interface ContentBlockText {
  "text": string;
  "type": "text";
}

export interface ContentBlockThinking {
  "redacted"?: boolean;
  "signature"?: string | null;
  "text": string;
  "type": "thinking";
}

export interface ContentBlockToolUse {
  "id": string;
  "input": unknown;
  "name": string;
  "type": "tool_use";
}

export interface ContentBlockToolResult {
  "content": Array<ToolResultBlock>;
  "is_error"?: boolean;
  "tool_use_id": string;
  "type": "tool_result";
}

export interface ContentBlockImage {
  "data": string;
  "media_type": string;
  "type": "image";
}

/**
 * An image a person sent with a message, kept by the daemon. Durable
 * state carries this reference; the bytes are read back only to build
 * a model request.
 */
export interface ContentBlockImageRef {
  "id": AttachmentId;
  "media_type": string;
  "name"?: string | null;
  "size_bytes": number;
  "type": "image_ref";
}

/**
 * A server-side compaction summary, replayed verbatim. Signed summaries
 * must lead the request, with the summarized history removed.
 */
export interface ContentBlockCompaction {
  "content": string;
  "signature"?: string | null;
  "type": "compaction";
}

export type ContentBlock =
  | ContentBlockText
  | ContentBlockThinking
  | ContentBlockToolUse
  | ContentBlockToolResult
  | ContentBlockImage
  | ContentBlockImageRef
  | ContentBlockCompaction;

/**
 * One rendered layer. System layers carry text; the tail carries messages
 * and the tool layer carries schemas, so those keep only counts here.
 */
export interface ContextLayer {
  "items"?: number;
  "kind": LayerKind;
  "memories"?: Array<MemoryId>;
  "placement"?: LayerPlacement;
  "text"?: string | null;
  "tokens": number;
}

/**
 * Where dollar figures came from; independent of whether every invocation was priced.
 */
export type CostBasis = "list_price" | "provider_reported" | "mixed" | "unknown";

/**
 * Why the dollar total may omit work.
 */
export type CostGap = "unreported" | "unpriced" | "interrupted" | "mixed" | "unknown";

/**
 * Request to create an agent, optionally with an initial task.
 * A child requires explicit `config.effort`: low, medium, high, xhigh or max.
 */
export interface CreateAgent {
  "approval"?: ApprovalPolicy | null;
  "config"?: AgentConfig | null;
  "fallbacks"?: Array<ModelRef>;
  "idempotency_key"?: string | null;
  "model"?: ModelRef | null;
  "name"?: string | null;
  "objective"?: string | null;
  "parent"?: AgentId | null;
  "repository_root"?: string | null;
  "role": string;
  "run"?: RunId | null;
  "scope"?: Scope | null;
  "start"?: boolean;
}

/**
 * On the wire `target` and `spawn` are sibling fields, exactly one set, so
 * a deliver request keeps the shape older clients send.
 */
export interface CreateScheduleSpec {
  "busy"?: BusyPolicy;
  "max_deliveries"?: number | null;
  "missed"?: MissedRunPolicy;
  "name": string;
  "payload"?: unknown;
  "recurrence": Recurrence;
  "spawn"?: SpawnSpec | null;
  "target"?: AgentId | null;
}

export interface CreateTask {
  "dependencies"?: Array<TaskId>;
  "idempotency_key"?: string | null;
  "objective": string;
  "owner"?: AgentId | null;
  "parent_task"?: TaskId | null;
  "priority"?: Priority;
}

export interface DaemonStatus {
  "agents_blocked": number;
  "agents_live": number;
  "agents_paused"?: number;
  "agents_running": number;
  "agents_suspended"?: number;
  "agents_waiting": number;
  "approval_default"?: ApprovalDefault;
  "bridge"?: string | null;
  "default_model"?: ModelRef | null;
  "engine_turns_running"?: number;
  "engines"?: Array<string>;
  "instance_id": string;
  "last_seq": number;
  "low_disk"?: LowDisk | null;
  "max_concurrent_engine_turns"?: number | null;
  "max_concurrent_turns"?: number | null;
  "mcp"?: Array<McpServerHealth>;
  "network_default"?: NetworkPolicy;
  "pending_approvals": number;
  "pid": number;
  "problems"?: Array<string>;
  "providers"?: Array<ProviderHealth>;
  "resource_leases"?: Array<ResourceHolding>;
  "run_budget_usd"?: number | null;
  "run_hard_stop_tokens"?: number | null;
  "run_hard_stop_usd"?: number | null;
  "runs"?: Array<RunSummary>;
  "stale"?: StatusStale | null;
  "started_at": string;
  "tasks_active": number;
  "usage_total": Usage;
  "version": string;
  "writes"?: WriteStats | null;
  "yolo"?: boolean;
}

/**
 * A change to an agent's wall-clock stop (§3.1). Every knob has to be
 * changeable on a live agent, and one that can only ever move further into
 * the future is not: removing the deadline needs a form of its own.
 */
export type DeadlineUpdate = "clear" | Record<string, unknown>;

/**
 * What deleting a session does to the propositions it produced.
 */
export type DeleteSessionGraph = "preserve" | "delete_nodes";

/**
 * What deleting a source does to the propositions it produced.
 */
export type DeleteSourceGraph = "preserve" | "delete_unsupported";

/**
 * Transient streaming signal. Not persisted; durable events reconcile it.
 * Tagged by `delta`.
 */
export interface DeltaTurnStarted {
  "delta": "turn_started";
  "turn": number;
}

export interface DeltaText {
  "delta": "text";
  "text": string;
}

export interface DeltaThinking {
  "delta": "thinking";
  "text": string;
}

export interface DeltaToolInput {
  "delta": "tool_input";
  "name": string;
  "partial_json": string;
  "tool_use_id"?: string | null;
}

/**
 * Absolute in-memory figures. Replace prior totals; never add these to durable usage.
 */
export interface DeltaAccounting {
  "context_tokens"?: number | null;
  "delta": "accounting";
  "total": Usage;
  "turn_usage": Usage;
}

export interface DeltaTurnFinished {
  "delta": "turn_finished";
}

export type Delta =
  | DeltaTurnStarted
  | DeltaText
  | DeltaThinking
  | DeltaToolInput
  | DeltaAccounting
  | DeltaTurnFinished;

/**
 * One enrolled device as `aether remote devices` and a phone's settings list it.
 */
export interface Device {
  "connections"?: number;
  "created_at": string;
  "current"?: boolean;
  "fingerprint"?: string | null;
  "id": DeviceId;
  "last_seen_at"?: string | null;
  "name": string;
  "push"?: boolean;
  "revoked_at"?: string | null;
  "role": DeviceRole;
}

/**
 * A dev_ prefixed ULID.
 * Pattern: ^dev_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type DeviceId = string;

/**
 * The device a connection authenticated as, echoed in [`crate::Welcome`].
 */
export interface DeviceIdentity {
  "id": DeviceId;
  "name": string;
  "role": DeviceRole;
}

/**
 * A device's answer to the carrier's challenge, sent in [`crate::Hello`].
 */
export interface DeviceProof {
  "id": DeviceId;
  "signature": string;
}

/**
 * What a paired device may do. See the authorization matrix in docs/remote.md.
 */
export type DeviceRole = "operator" | "observer";

export interface DisablePatch {
  "grace_seconds"?: number | null;
}

/**
 * What a disable does to work in flight.
 */
export interface DisablePolicy {
  "background": BackgroundDisable;
  "grace_seconds": number;
}

export interface DisableSettings {
  "grace_seconds": number;
}

/**
 * A constraint the compactor decided no longer applies, with its reason.
 */
export interface DroppedItem {
  "reason": string;
  "text": string;
}

/**
 * Reasoning depth. Providers map this to their own controls.
 */
export type Effort = "low" | "medium" | "high" | "x_high" | "max";

/**
 * How one agent or task ended.
 * Tagged by `outcome`.
 */
export interface EndedRunning {
  "outcome": "running";
}

export interface EndedCompleted {
  "outcome": "completed";
  "summary"?: string | null;
}

export interface EndedFailed {
  "error": string;
  "outcome": "failed";
}

export interface EndedCancelled {
  "outcome": "cancelled";
  "reason"?: string | null;
}

export type Ended = EndedRunning | EndedCompleted | EndedFailed | EndedCancelled;

/**
 * The per-agent context layers an engine session has already been sent, as
 * digests of their rendered text.
 *
 * An engine session ordinarily retains every user turn it was sent, so a
 * layer whose text has not changed since it was last sent is normally
 * already in front of the model; sending it again would duplicate it in a
 * conversation that keeps both copies. That guarantee ends at the
 * engine's own context compaction - Claude Code's recorded system prompt is
 * reused only "until the conversation is compacted" - which can replace a
 * retained turn with a summary the model never sees again as the original
 * text. Aether requests its own compaction first in the common case, and
 * `turn_engine` clears this record on a sharp context drop as a cheap bound
 * against the other order, but neither proves the engine's compaction never
 * wins the race.
 *
 * The session id is part of the record: a cleared or replaced session
 * retains nothing, so its digests must not gate the next turn.
 */
export interface EngineSentLayers {
  "layers": Record<string, unknown>;
  "session": string;
}

/**
 * An engine session's own cumulative token counters, as it last reported
 * them, tagged with the session they belong to.
 *
 * Codex counts a *thread*, not a turn: the counters it reports on a resumed
 * thread include every earlier turn. Persisting the last reading is what
 * lets the next turn bill `total - this` instead of the thread's history.
 * The session id is part of the record so a cleared or replaced session
 * cannot be billed against another session's counters.
 */
export interface EngineSessionUsage {
  "models"?: Record<string, Usage> | null;
  "session": string;
  "total": Usage;
}

export type EngineeringExtraction = "off" | "auto";

export type ErrorCode = "not_found" | "invalid_transition" | "invalid_argument" | "internal" | "incompatible" | "unauthorized" | "unavailable" | "forbidden" | "unimplemented" | "no_model" | "history_expired";

export interface ErrorReply {
  "code": ErrorCode;
  "first_seq"?: number | null;
  "last_seq"?: number | null;
  "message": string;
  "reason"?: string | null;
}

/**
 * One normalized error class a provider returned recently, and how often.
 */
export interface ErrorTally {
  "class": ModelErrorClass;
  "count": number;
  "last_at": string;
}

/**
 * A committed event.
 */
export interface Event {
  "actor": Actor;
  "agent_id"?: AgentId | null;
  "at": string;
  "causation"?: EventId | null;
  "id": EventId;
  "kind": EventKind;
  "parent_agent_id"?: AgentId | null;
  "run"?: RunId | null;
  "seq": number;
  "task_id"?: TaskId | null;
}

/**
 * A evt_ prefixed ULID.
 * Pattern: ^evt_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type EventId = string;

/**
 * Typed event payloads, grouped by family. The serialized `type` tag uses
 * dotted names (`agent.created`) so filters can match by family prefix.
 * Tagged by `type`.
 */
export interface EventKindAgentCreated {
  "fallbacks"?: Array<ModelRef>;
  "model": ModelRef;
  "name": string;
  "parent"?: AgentId | null;
  "role": Role;
  "scope": Scope;
  "type": "agent.created";
}

export interface EventKindAgentReady {
  "type": "agent.ready";
}

export interface EventKindAgentStarted {
  "turn": number;
  "type": "agent.started";
}

export interface EventKindAgentTurnCompleted {
  "invocation"?: InvocationId | null;
  "next": AgentStatusKind;
  "turn": number;
  "type": "agent.turn_completed";
}

/**
 * The operator stopped the running turn. The agent keeps its
 * transcript and waits for the next message.
 */
export interface EventKindAgentInterrupted {
  "turn": number;
  "type": "agent.interrupted";
}

export interface EventKindAgentWaiting {
  "condition": WaitCondition;
  "type": "agent.waiting";
}

export interface EventKindAgentResumed {
  "reason": string;
  "type": "agent.resumed";
  "via"?: AgentId | null;
}

/**
 * The operator paused the agent (VISION §3.1). Durable and orthogonal to
 * the status: it takes no turns until resumed, and its inbox fills.
 */
export interface EventKindAgentPaused {
  "type": "agent.paused";
  "via"?: AgentId | null;
}

/**
 * The agent was cleared out of the live view, or brought back.
 * A view mark: nothing is removed, and replay is unaffected.
 */
export interface EventKindAgentArchived {
  "archived": boolean;
  "type": "agent.archived";
}

export interface EventKindAgentBlocked {
  "dependency"?: TaskId | null;
  "reason": string;
  "type": "agent.blocked";
}

export interface EventKindAgentCompleted {
  "summary"?: string | null;
  "type": "agent.completed";
}

export interface EventKindAgentFailed {
  "error": string;
  "type": "agent.failed";
}

/**
 * A failed or cancelled agent was recreated from its last checkpoint
 * (§3.8). Recorded on both agents so either end of the link is visible
 * from the other's event stream.
 */
export interface EventKindAgentRestarted {
  "checkpoint"?: CheckpointId | null;
  "dropped_deadline"?: string | null;
  "from": AgentId;
  "inbox_carried"?: number;
  "to": AgentId;
  "transcript_dropped"?: number;
  "transcript_tail": number;
  "type": "agent.restarted";
}

export interface EventKindAgentCancelled {
  "reason"?: string | null;
  "type": "agent.cancelled";
}

/**
 * An operator or orchestrator changed the agent's configuration mid-run
 * (model, effort, tool exposure, approval policy, budget).
 */
export interface EventKindAgentConfigUpdated {
  "config": AgentConfig;
  "model"?: ModelRef | null;
  "name"?: string | null;
  "scope"?: Scope | null;
  "type": "agent.config_updated";
}

/**
 * Absolute billing totals, including corrections from a completed engine run.
 */
export interface EventKindAgentUsageUpdated {
  "total": Usage;
  "turn_usage": Usage;
  "type": "agent.usage_updated";
}

/**
 * A transcript entry: what the model said, or what it was shown.
 */
export interface EventKindAgentMessage {
  "content": Array<ContentBlock>;
  "invocation"?: InvocationId | null;
  "role": MessageRole;
  "type": "agent.message";
}

export interface EventKindTaskCreated {
  "dependencies"?: Array<TaskId>;
  "objective": string;
  "parent_task"?: TaskId | null;
  "priority": Priority;
  "type": "task.created";
}

export interface EventKindTaskAssigned {
  "owner": AgentId;
  "type": "task.assigned";
}

export interface EventKindTaskStarted {
  "type": "task.started";
}

export interface EventKindTaskBlocked {
  "dependency"?: TaskId | null;
  "reason": string;
  "type": "task.blocked";
}

export interface EventKindTaskUnblocked {
  "type": "task.unblocked";
}

export interface EventKindTaskRequeued {
  "reason": string;
  "type": "task.requeued";
}

export interface EventKindTaskCompleted {
  "result": TaskResult;
  "type": "task.completed";
}

export interface EventKindTaskFailed {
  "error": string;
  "structured"?: unknown;
  "type": "task.failed";
}

export interface EventKindTaskCancelled {
  "reason"?: string | null;
  "type": "task.cancelled";
}

export interface EventKindToolRequested {
  "call": ToolCall;
  "side_effect": SideEffectClass;
  "type": "tool.requested";
}

export interface EventKindToolStarted {
  "call": ToolCallId;
  "type": "tool.started";
}

export interface EventKindToolSucceeded {
  "call": ToolCallId;
  "outcome": ToolOutcome;
  "type": "tool.succeeded";
}

export interface EventKindToolFailed {
  "call": ToolCallId;
  "duration_ms"?: number | null;
  "error": string;
  "retryable": boolean;
  "type": "tool.failed";
}

export interface EventKindToolCancelled {
  "call": ToolCallId;
  "type": "tool.cancelled";
}

export interface EventKindToolDenied {
  "call": ToolCallId;
  "reason": string;
  "type": "tool.denied";
}

/**
 * The call carried an idempotency key already committed for this agent,
 * so the recorded outcome was returned instead of running again.
 */
export interface EventKindToolReplayed {
  "call": ToolCallId;
  "key": IdempotencyKey;
  "original": ToolCallId;
  "type": "tool.replayed";
}

/**
 * A retryable failure is being tried again, within the class's policy.
 */
export interface EventKindToolRetried {
  "attempt": number;
  "call": ToolCallId;
  "error": string;
  "of": number;
  "type": "tool.retried";
}

export interface EventKindSnapshotCreated {
  "layers": Array<ContextLayer>;
  "model": ModelRef;
  "snapshot": SnapshotId;
  "token_estimate": number;
  "turn": number;
  "type": "snapshot.created";
}

export interface EventKindInvocationCompleted {
  "duration_ms": number;
  "invocation": InvocationId;
  "model": ModelRef;
  "snapshot": SnapshotId;
  "stop_reason"?: StopReason | null;
  "type": "invocation.completed";
  "usage": Usage;
}

/**
 * The turn did not complete. Streamed content accumulated before the
 * failure or cancellation is kept so nothing the user watched is lost.
 */
export interface EventKindInvocationFailed {
  "cancelled"?: boolean;
  "class"?: ModelErrorClass | null;
  "error": string;
  "invocation": InvocationId;
  "partial"?: Array<ContentBlock>;
  "retryable": boolean;
  "snapshot": SnapshotId;
  "type": "invocation.failed";
}

export interface EventKindCompactionRequested {
  "trigger": CompactionTrigger;
  "type": "compaction.requested";
}

export interface EventKindCheckpointCreated {
  "checkpoint": Checkpoint;
  "type": "checkpoint.created";
}

export interface EventKindCompactionCompleted {
  "after_tokens"?: number | null;
  "before_tokens"?: number | null;
  "checkpoint": CheckpointId;
  "from_seq": number;
  "to_seq": number;
  "type": "compaction.completed";
  "version": number;
}

/**
 * The engine or provider serving the agent folded the context itself
 * (Claude Code's `/compact`, Anthropic's server-side summary). No
 * checkpoint exists: the summary lives in the engine session or the transcript.
 */
export interface EventKindCompactionNative {
  "after_tokens"?: number | null;
  "automatic"?: boolean;
  "before_tokens"?: number | null;
  "duration_ms"?: number | null;
  "provider": string;
  "type": "compaction.native";
}

/**
 * `models` names what was asked to fold, in the order it was tried: a
 * transcript that reached a provider that then failed leaves no other
 * durable trace of the egress.
 */
export interface EventKindCompactionFailed {
  "error": string;
  "models"?: Array<ModelRef>;
  "type": "compaction.failed";
}

/**
 * An agent's context crossed the engine/API boundary. Names what carried
 * over and, honestly, what did not.
 */
export interface EventKindContextHandoff {
  "carried": number;
  "checkpoint"?: CheckpointId | null;
  "from": ModelRef;
  "lost"?: Array<string>;
  "to": ModelRef;
  "type": "context.handoff";
}

export interface EventKindMemoryCandidate {
  "candidate": MemoryCandidate;
  "type": "memory.candidate";
}

export interface EventKindMemoryAccepted {
  "memory": MemoryId;
  "reason"?: string | null;
  "type": "memory.accepted";
}

export interface EventKindMemoryRejected {
  "reason": string;
  "type": "memory.rejected";
}

/**
 * Curated out by a person. The record stays; it is never retrieved again.
 */
export interface EventKindMemoryForgotten {
  "memory": MemoryId;
  "reason"?: string | null;
  "type": "memory.forgotten";
}

/**
 * Put back after a `memory.forgotten`, so a mis-keyed curation has a way
 * back that leaves the same trail everything else does.
 */
export interface EventKindMemoryRestored {
  "memory": MemoryId;
  "type": "memory.restored";
}

/**
 * Pinned in front of the model, or unpinned.
 */
export interface EventKindMemoryPinned {
  "memory": MemoryId;
  "pinned": boolean;
  "type": "memory.pinned";
}

export interface EventKindMemorySuperseded {
  "new": MemoryId;
  "old": MemoryId;
  "reason"?: string | null;
  "type": "memory.superseded";
}

export interface EventKindMessageSent {
  "content": Array<ContentBlock>;
  "from": Actor;
  "message": MessageId;
  "to": AgentId;
  "type": "message.sent";
}

export interface EventKindInboxDelivered {
  "item": InboxItem;
  "type": "inbox.delivered";
}

export interface EventKindInboxConsumed {
  "items": Array<InboxItemId>;
  "type": "inbox.consumed";
  "up_to": number;
}

/**
 * `ask_user` durably recording what it asked, so a client can show it
 * and a person answering later still has it in view (the reply itself
 * is an ordinary `message.sent` back to this agent).
 */
export interface EventKindQuestionAsked {
  "question": string;
  "type": "question.asked";
}

export interface EventKindApprovalRequested {
  "approval": ApprovalId;
  "argument"?: string | null;
  "call": ToolCallId;
  "description": string;
  "side_effect": SideEffectClass;
  "tool": string;
  "type": "approval.requested";
}

/**
 * A decision was remembered as a durable rule.
 */
export interface EventKindApprovalRuleAdded {
  "rule": ApprovalRule;
  "type": "approval.rule_added";
}

/**
 * The rule stops deciding anything from here on.
 */
export interface EventKindApprovalRuleRevoked {
  "revocation": Revocation;
  "rule": ApprovalRuleId;
  "type": "approval.rule_revoked";
}

export interface EventKindApprovalResolved {
  "approval": ApprovalId;
  "by": Actor;
  "decision": ApprovalDecision;
  "note"?: string | null;
  "type": "approval.resolved";
}

export interface EventKindArtifactCreated {
  "artifact": Artifact;
  "type": "artifact.created";
}

/**
 * An identical artifact already existed, so the bytes were not stored
 * again and the call was given the existing one.
 */
export interface EventKindArtifactDeduplicated {
  "artifact": ArtifactId;
  "digest": string;
  "shared_with"?: ArtifactId | null;
  "type": "artifact.deduplicated";
}

/**
 * Retention dropped the bytes. The record stays; the payload is gone.
 */
export interface EventKindArtifactEvicted {
  "artifact": ArtifactId;
  "reason": string;
  "size_bytes": number;
  "type": "artifact.evicted";
}

/**
 * A configured MCP server answered and its tools are offered to agents.
 */
export interface EventKindMcpServerConnected {
  "server": string;
  "tools": Array<string>;
  "type": "mcp.server_connected";
}

/**
 * A configured MCP server could not be reached or stopped answering. Its
 * tools are withdrawn; a call against it fails rather than hanging.
 */
export interface EventKindMcpServerFailed {
  "error": string;
  "server": string;
  "type": "mcp.server_failed";
}

/**
 * A run was replayed into this store from another daemon's export. It is
 * the imported run's first event, so everything after it reads as
 * history that happened elsewhere.
 */
export interface EventKindRunImported {
  "events": number;
  "exported_at": string;
  "source_instance": string;
  "source_run": RunId;
  "type": "run.imported";
}

export interface EventKindLeaseGranted {
  "lease": Lease;
  "type": "lease.granted";
}

export interface EventKindLeaseRenewed {
  "expires_at": string;
  "type": "lease.renewed";
}

export interface EventKindLeaseReleased {
  "type": "lease.released";
}

export interface EventKindLeaseExpired {
  "holder": string;
  "type": "lease.expired";
}

/**
 * A turn is holding resource keys for its mutating tool calls.
 */
export interface EventKindLeaseResourceAcquired {
  "keys": Array<ResourceKey>;
  "type": "lease.resource_acquired";
}

export interface EventKindLeaseResourceReleased {
  "keys": Array<ResourceKey>;
  "type": "lease.resource_released";
}

/**
 * Another agent holds a key this turn needs; it waits, visibly Blocked.
 */
export interface EventKindLeaseResourceWaiting {
  "blocked_on": ResourceKey;
  "holder": AgentId;
  "keys": Array<ResourceKey>;
  "type": "lease.resource_waiting";
}

/**
 * The scheduler is holding turns back from a provider: a 429, a
 * repeated failure, or a subscription window near its limit.
 */
export interface EventKindProviderBackoff {
  "provider": string;
  "reason": string;
  "type": "provider.backoff";
  "until": string;
}

export interface EventKindProviderRecovered {
  "provider": string;
  "type": "provider.recovered";
}

/**
 * A subscription engine reported how much of its window is spent.
 * A turn moved to the next link of its model chain (VISION §4.5).
 */
export interface EventKindProviderFallback {
  "class"?: ModelErrorClass | null;
  "from": ModelRef;
  "reason": string;
  "to": ModelRef;
  "type": "provider.fallback";
}

export interface EventKindProviderQuota {
  "provider": string;
  "type": "provider.quota";
  "usage"?: ProviderUsage | null;
  "window": QuotaWindow;
}

export interface EventKindRecoveryClassified {
  "class": RecoveryClass;
  "detail": string;
  "type": "recovery.classified";
}

/**
 * A budget is 80 % spent: the person is told before the stop.
 */
export interface EventKindBudgetWarning {
  "reason": string;
  "type": "budget.warning";
  "usage": Usage;
}

/**
 * The agent's wall-clock deadline passed; it stops as Failed.
 */
export interface EventKindDeadlinePassed {
  "reason": string;
  "type": "deadline.passed";
}

/**
 * A budget is spent; the agent stops as Failed with this reason.
 */
export interface EventKindBudgetExceeded {
  "reason": string;
  "type": "budget.exceeded";
  "usage": Usage;
}

/**
 * A run crossed its hard spend stop: every live agent in it was
 * interrupted (not failed) and no new turn is admitted until the limit
 * is raised.
 */
export interface EventKindRunHardStop {
  "interrupted": number;
  "limit": string;
  "reason": string;
  "type": "run.hard_stop";
  "usage": Usage;
}

export interface EventKindDaemonStarted {
  "pid": number;
  "type": "daemon.started";
  "version": string;
}

export interface EventKindDaemonStopped {
  "background_processes_stopped"?: number;
  "type": "daemon.stopped";
}

export interface EventKindNoesisSessionCreated {
  "session": NoesisSession;
  "type": "noesis.session_created";
}

export interface EventKindNoesisPhaseChanged {
  "from": Phase;
  "session": string;
  "to": Phase;
  "type": "noesis.phase_changed";
}

export interface EventKindNoesisQuestionPosed {
  "question": Question;
  "session": string;
  "type": "noesis.question_posed";
}

export interface EventKindNoesisAnswerRecorded {
  "result": QuestionResult;
  "session": string;
  "type": "noesis.answer_recorded";
}

export interface EventKindNoesisPlanUpdated {
  "plan": PlanTree;
  "session": string;
  "type": "noesis.plan_updated";
}

export interface EventKindNoesisProblemPosed {
  "problem": Problem;
  "session": string;
  "type": "noesis.problem_posed";
}

/**
 * The only place a problem's sealed solution is revealed.
 */
export interface EventKindNoesisPracticeConcluded {
  "miss_class"?: MissClass | null;
  "outcome": PracticeOutcome;
  "problem": string;
  "retest": boolean;
  "rung"?: number | null;
  "session": string;
  "solution": string;
  "type": "noesis.practice_concluded";
}

export interface EventKindNoesisJobUpdated {
  "job": Job;
  "type": "noesis.job_updated";
}

export interface EventKindNoesisGraphMerged {
  "job": string;
  "stats": JobStats;
  "type": "noesis.graph_merged";
}

export interface EventKindNoesisLearnerUpdated {
  "concepts": Array<string>;
  "type": "noesis.learner_updated";
}

export interface EventKindNoesisSettingsChanged {
  "effective": NoesisSettingsView;
  "settings": NoesisSettingsView;
  "type": "noesis.settings_changed";
}

/**
 * Escape hatch for orchestration layers to record their own events.
 */
export interface EventKindCustom {
  "name": string;
  "payload": unknown;
  "type": "custom";
}

export type EventKind =
  | EventKindAgentCreated
  | EventKindAgentReady
  | EventKindAgentStarted
  | EventKindAgentTurnCompleted
  | EventKindAgentInterrupted
  | EventKindAgentWaiting
  | EventKindAgentResumed
  | EventKindAgentPaused
  | EventKindAgentArchived
  | EventKindAgentBlocked
  | EventKindAgentCompleted
  | EventKindAgentFailed
  | EventKindAgentRestarted
  | EventKindAgentCancelled
  | EventKindAgentConfigUpdated
  | EventKindAgentUsageUpdated
  | EventKindAgentMessage
  | EventKindTaskCreated
  | EventKindTaskAssigned
  | EventKindTaskStarted
  | EventKindTaskBlocked
  | EventKindTaskUnblocked
  | EventKindTaskRequeued
  | EventKindTaskCompleted
  | EventKindTaskFailed
  | EventKindTaskCancelled
  | EventKindToolRequested
  | EventKindToolStarted
  | EventKindToolSucceeded
  | EventKindToolFailed
  | EventKindToolCancelled
  | EventKindToolDenied
  | EventKindToolReplayed
  | EventKindToolRetried
  | EventKindSnapshotCreated
  | EventKindInvocationCompleted
  | EventKindInvocationFailed
  | EventKindCompactionRequested
  | EventKindCheckpointCreated
  | EventKindCompactionCompleted
  | EventKindCompactionNative
  | EventKindCompactionFailed
  | EventKindContextHandoff
  | EventKindMemoryCandidate
  | EventKindMemoryAccepted
  | EventKindMemoryRejected
  | EventKindMemoryForgotten
  | EventKindMemoryRestored
  | EventKindMemoryPinned
  | EventKindMemorySuperseded
  | EventKindMessageSent
  | EventKindInboxDelivered
  | EventKindInboxConsumed
  | EventKindQuestionAsked
  | EventKindApprovalRequested
  | EventKindApprovalRuleAdded
  | EventKindApprovalRuleRevoked
  | EventKindApprovalResolved
  | EventKindArtifactCreated
  | EventKindArtifactDeduplicated
  | EventKindArtifactEvicted
  | EventKindMcpServerConnected
  | EventKindMcpServerFailed
  | EventKindRunImported
  | EventKindLeaseGranted
  | EventKindLeaseRenewed
  | EventKindLeaseReleased
  | EventKindLeaseExpired
  | EventKindLeaseResourceAcquired
  | EventKindLeaseResourceReleased
  | EventKindLeaseResourceWaiting
  | EventKindProviderBackoff
  | EventKindProviderRecovered
  | EventKindProviderFallback
  | EventKindProviderQuota
  | EventKindRecoveryClassified
  | EventKindBudgetWarning
  | EventKindDeadlinePassed
  | EventKindBudgetExceeded
  | EventKindRunHardStop
  | EventKindDaemonStarted
  | EventKindDaemonStopped
  | EventKindNoesisSessionCreated
  | EventKindNoesisPhaseChanged
  | EventKindNoesisQuestionPosed
  | EventKindNoesisAnswerRecorded
  | EventKindNoesisPlanUpdated
  | EventKindNoesisProblemPosed
  | EventKindNoesisPracticeConcluded
  | EventKindNoesisJobUpdated
  | EventKindNoesisGraphMerged
  | EventKindNoesisLearnerUpdated
  | EventKindNoesisSettingsChanged
  | EventKindCustom;

export interface EventQuery {
  "agent"?: AgentId | null;
  "before"?: number | null;
  "child_types"?: Array<string>;
  "families"?: Array<string>;
  "run"?: RunId | null;
  "task"?: TaskId | null;
  "types"?: Array<string>;
}

/**
 * One append-only row of the learner's history.
 */
export interface Evidence {
  "at": string;
  "concept": string;
  "confidence"?: number | null;
  "hint_rung"?: number | null;
  "id": number;
  "kind": EvidenceKind;
  "miss_class"?: MissClass | null;
  "mode"?: ProblemMode | null;
  "note"?: string | null;
  "outcome": EvidenceOutcome;
  "session"?: string | null;
}

/**
 * What the frontend gathered from reads the user can already make, stored
 * with the session that starts from it.
 */
export interface EvidenceBundle {
  "items": Array<EvidenceItem>;
  "title": string;
}

export interface EvidenceItem {
  "excerpt": string;
  "ref": EvidenceRef;
  "status": EvidenceStatus;
}

export type EvidenceKind = "probe_answer" | "predict_answer" | "check_answer" | "recall_judged" | "practice_conclude" | "exposure" | "misconception_observed" | "operator_correction";

export type EvidenceOutcome = "correct" | "incorrect" | "dont_know" | "exposed" | "observed" | "corrected";

export interface EvidenceRef {
  "id": string;
  "kind": EvidenceRefKind;
  "url"?: string | null;
}

export type EvidenceRefKind = "pr" | "issue" | "item" | "run" | "stage" | "commit" | "file" | "agent";

export type EvidenceStatus = "verified" | "reviewed" | "preliminary";

/**
 * Where an agent's tools actually run.
 * Tagged by `backend`.
 */
/**
 * The daemon's own machine, inside the scoped roots.
 */
export interface ExecutionBackendHost {
  "backend": "host";
}

/**
 * A container per call, with the workspace mounted writable at the same
 * path, everything else in scope mounted read-only, and network per the
 * scope's policy.
 */
export interface ExecutionBackendContainer {
  "backend": "container";
  "image": string;
}

export type ExecutionBackend = ExecutionBackendHost | ExecutionBackendContainer;

/**
 * The subset of [`ModelErrorClass`] a probe can recover from. `ContextLimit`
 * is deliberately unrepresentable: it is not credits, and waiting cannot fix it.
 * `Credentials` is a sign-in the provider refused, which an operator renews.
 */
export type ExhaustionClass = "quota_exhausted" | "usage_limit" | "credentials";

export interface ExplainedTurn {
  "agent": AgentId;
  "at": string;
  "candidates"?: Array<TraceCandidate>;
  "injected"?: Array<MemoryId>;
  "invocations"?: Array<InvocationId>;
  "omitted"?: TraceOmitted;
  "policy"?: TracePolicy | null;
  "pool": number;
  "snapshot": SnapshotId;
  "task"?: TaskId | null;
  "traced": boolean;
  "turn": number;
}

/**
 * What an export wrote.
 */
export interface ExportSummary {
  "agents": number;
  "artifact_payloads": number;
  "artifacts": number;
  "bytes_written": number;
  "checkpoints": number;
  "events": number;
  "invocations": number;
  "path": string;
  "run": RunId;
  "snapshots": number;
  "tasks": number;
}

export interface ExtractionPatch {
  "engineering"?: EngineeringExtraction | null;
  "sessions"?: SessionExtraction | null;
}

export interface ExtractionSettings {
  "engineering": EngineeringExtraction;
  "sessions": SessionExtraction;
}

/**
 * What failed, and what it was. Every failure in the run, in order.
 */
export interface FailureLine {
  "agent"?: AgentId | null;
  "at": string;
  "error": string;
  "kind": string;
}

/**
 * The last time a turn gave up on a provider and moved to the next link of
 * its chain.
 */
export interface FallbackRecord {
  "at": string;
  "class"?: ModelErrorClass | null;
  "reason": string;
  "to": ModelRef;
}

/**
 * A memory's current tally and the factor it gives, as the host reports it.
 */
export interface FeedbackRanking {
  "factor": number;
  "helped": number;
  "hurt": number;
}

export type FileStatus = "added" | "modified" | "deleted";

export interface GitSummary {
  "ahead"?: number | null;
  "behind"?: number | null;
  "branch"?: string | null;
  "detached"?: boolean;
  "dirty"?: boolean;
  "head"?: string | null;
  "modified"?: number;
  "repository": boolean;
  "root"?: string | null;
  "staged"?: number;
  "untracked"?: number;
}

/**
 * How a repository's accepted requests reach an agent.
 * Tagged by `mode`.
 */
/**
 * Deliver every trigger to the fixed, operator-chosen `agent`. Stale if
 * that agent pauses or goes terminal; an operator must repoint it.
 */
export interface GithubDispatchModeFixed {
  "mode": "fixed";
}

/**
 * Create an Aether-managed checkout and agent for each issue/PR thread.
 * The daemon's current model, approval, and network defaults apply.
 */
export interface GithubDispatchModeAutomatic {
  "mode": "automatic";
}

/**
 * Deliver to (or spawn, the first time or after the previous one went
 * terminal) a durable agent keyed by (repository, issue/PR number), so
 * the watcher heals itself instead of pointing at a dead agent forever.
 */
export interface GithubDispatchModeThread {
  "approval"?: ApprovalPolicy | null;
  "config"?: AgentConfig | null;
  "fallbacks"?: Array<ModelRef>;
  "mode": "thread";
  "model"?: ModelRef | null;
  "name"?: string | null;
  "objective": string;
  "role": string;
  "scope"?: Scope | null;
}

export type GithubDispatchMode =
  | GithubDispatchModeFixed
  | GithubDispatchModeAutomatic
  | GithubDispatchModeThread;

export interface GithubRepositoryConfig {
  "agent"?: AgentId | null;
  "allowed_users"?: Array<string> | null;
  "dispatch"?: GithubDispatchMode;
  "enabled": boolean;
  "repository": string;
}

export interface GithubRepositoryStatus {
  "dispatched": number;
  "ignored": number;
  "last_error"?: string | null;
  "last_poll_at"?: string | null;
  "last_success_at"?: string | null;
  "next_poll_at"?: string | null;
  "repository": string;
}

export interface GithubWatcherConfig {
  "enabled"?: boolean;
  "label_regex"?: string;
  "mention_regex"?: string;
  "operator_users"?: Array<string> | null;
  "poll_interval_secs"?: number;
  "repositories"?: Array<GithubRepositoryConfig>;
}

export interface GithubWatcherStatus {
  "config": GithubWatcherConfig;
  "repositories": Array<GithubRepositoryStatus>;
  "revision": number;
}

/**
 * Body of `GET /api/health`.
 */
export interface Health {
  "min_protocol_version": number;
  "pairing_open": boolean;
  "protocol_version": number;
  "status": string;
}

export interface Hello {
  "client_name": string;
  "client_version": string;
  "device"?: DeviceProof | null;
  "features"?: Array<string>;
  "protocol_version": number;
  "token"?: string | null;
}

/**
 * Engine host setup a session loads. Lean drops Claude Code's native skills
 * and non-Aether MCP servers; other engines ignore it.
 */
export type HostContext = "full" | "lean";

/**
 * Every non-2xx HTTP response body: `{"error": {"code", "message"}}`.
 */
export interface HttpError {
  "error": HttpErrorBody;
}

export interface HttpErrorBody {
  "code": HttpErrorCode;
  "message": string;
}

/**
 * Why an HTTP request was refused. See the table in docs/remote.md.
 */
export type HttpErrorCode = "bad_request" | "forbidden_host" | "forbidden_origin" | "not_found" | "method_not_allowed" | "unsupported_media_type" | "payload_too_large" | "pairing_closed" | "invalid_code" | "expired" | "rate_limited" | "unavailable" | "internal";

/**
 * What makes two calls the same effect.
 */
export type IdempotencyKey = string;

/**
 * One image as a client sends it with a `send_message` command.
 */
export interface ImageAttachment {
  "data": string;
  "media_type": string;
  "name"?: string | null;
}

export interface ImportConflict {
  "memory": MemoryId;
  "reason": string;
}

/**
 * What an import recreated.
 */
export interface ImportSummary {
  "agents": number;
  "artifact_payloads": number;
  "artifacts": number;
  "checkpoints": number;
  "events": number;
  "exported_at": string;
  "invocations": number;
  "run": RunId;
  "snapshots": number;
  "source_instance": string;
  "source_run": RunId;
  "tasks": number;
}

/**
 * One delivered item. `seq` is per-agent and strictly increasing.
 */
export interface InboxItem {
  "agent_id": AgentId;
  "delivered_at": string;
  "id": InboxItemId;
  "kind": InboxKind;
  "seq": number;
}

/**
 * A ibx_ prefixed ULID.
 * Pattern: ^ibx_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type InboxItemId = string;

/**
 * Tagged by `kind`.
 */
export interface InboxKindTaskCompleted {
  "kind": "task_completed";
  "result": TaskResult;
  "task": TaskId;
}

export interface InboxKindTaskFailed {
  "error": string;
  "kind": "task_failed";
  "structured"?: unknown;
  "task": TaskId;
}

export interface InboxKindTaskCancelled {
  "kind": "task_cancelled";
  "reason"?: string | null;
  "task": TaskId;
}

export interface InboxKindMessage {
  "content": Array<ContentBlock>;
  "from": Actor;
  "kind": "message";
  "message_id": MessageId;
}

export interface InboxKindApprovalResolved {
  "approval": ApprovalId;
  "by"?: Actor | null;
  "decision": ApprovalDecision;
  "kind": "approval_resolved";
  "note"?: string | null;
}

export interface InboxKindChildFailed {
  "agent": AgentId;
  "error": string;
  "kind": "child_failed";
}

/**
 * A descendant parked on provider exhaustion; it probes and resumes by
 * itself, so the recipient is informed rather than asked to act.
 */
export interface InboxKindChildSuspended {
  "agent": AgentId;
  "detail": string;
  "kind": "child_suspended";
}

/**
 * The same descendant's provider answered again and its turn continues;
 * informational, so it never interrupts a wait.
 */
export interface InboxKindChildResumed {
  "agent": AgentId;
  "kind": "child_resumed";
}

export interface InboxKindExternalTrigger {
  "kind": "external_trigger";
  "operator"?: boolean;
  "payload": unknown;
  "source": string;
}

/**
 * A bare wake with a reason (deadline passed, operator nudge).
 */
export interface InboxKindWake {
  "kind": "wake";
  "reason": string;
}

export type InboxKind =
  | InboxKindTaskCompleted
  | InboxKindTaskFailed
  | InboxKindTaskCancelled
  | InboxKindMessage
  | InboxKindApprovalResolved
  | InboxKindChildFailed
  | InboxKindChildSuspended
  | InboxKindChildResumed
  | InboxKindExternalTrigger
  | InboxKindWake;

/**
 * The result of sending a snapshot to a model.
 */
export interface Invocation {
  "agent_id": AgentId;
  "content"?: Array<ContentBlock>;
  "error"?: string | null;
  "finished_at"?: string | null;
  "id": InvocationId;
  "model": ModelRef;
  "provider_message_id"?: string | null;
  "snapshot_id": SnapshotId;
  "started_at": string;
  "stop_reason"?: StopReason | null;
  "turn": number;
  "usage"?: Usage;
}

/**
 * A inv_ prefixed ULID.
 * Pattern: ^inv_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type InvocationId = string;

export interface InvocationParams {
  "effort"?: Effort | null;
  "extensions"?: Record<string, unknown>;
  "max_output_tokens"?: number | null;
  "temperature"?: number | null;
  "thinking"?: ThinkingMode | null;
}

/**
 * Everything sent to the model on one turn, frozen before the request.
 *
 * Remote inference is not deterministic, so this does not promise replay.
 * It promises reconstruction: every input the model received, and the
 * adapter and capability profile that shaped the wire request, are here.
 */
export interface InvocationSnapshot {
  "adapter"?: string | null;
  "agent_id": AgentId;
  "capabilities"?: CapabilityProfile | null;
  "created_at": string;
  "id": SnapshotId;
  "layers": Array<ContextLayer>;
  "memory_ids"?: Array<MemoryId>;
  "memory_retrieval"?: RetrievalTrace | null;
  "messages": Array<Message>;
  "model": ModelRef;
  "params": InvocationParams;
  "task_id"?: TaskId | null;
  "token_estimate": number;
  "tools": Array<ToolSpec>;
  "turn": number;
}

export interface Job {
  "created_at": string;
  "error"?: string | null;
  "id": string;
  "kind": JobKind;
  "state": JobState;
  "stats"?: JobStats | null;
  "target": string;
  "updated_at": string;
  "usage": Usage;
}

export type JobKind = "extract_session" | "extract_source" | "extract_engineering";

export type JobState = "queued" | "running" | "merging" | "succeeded" | "failed" | "cancelled";

export interface JobStats {
  "chains": number;
  "dropped_chains": number;
  "nodes": number;
}

/**
 * Rows and payload bytes written for one kind of record: an event type such
 * as `agent.usage_updated`, or a table such as `table:agents`.
 */
export interface KindWrites {
  "bytes": number;
  "kind": string;
  "rows": number;
}

export interface LastImport {
  "at": string;
  "conflicts": number;
  "digest": string;
  "inserted": number;
  "unchanged": number;
}

/**
 * The ordered context layers. Order is the order they are rendered.
 */
export type LayerKind = "harness_policy" | "model_profile" | "role_task" | "runtime_state" | "checkpoint" | "retrieved_memory" | "recent_tail" | "tool_schemas";

/**
 * Where a layer's text reached the model on this turn.
 */
export type LayerPlacement = "system" | "user_turn" | "retained";

export interface Lease {
  "agent_id": AgentId;
  "expires_at": string;
  "granted_at": string;
  "holder": string;
  "turn": number;
}

/**
 * The filesystem holding the daemon's data is below its floor: new turns
 * wait until it has room again. Turns already running carry on.
 */
export interface LowDisk {
  "available_bytes": number;
  "floor_bytes": number;
  "path": string;
  "since": string;
}

/**
 * How a server authenticates. Names the variable, never its value.
 */
export interface McpAuth {
  "state": McpAuthState;
  "token_env": string;
}

/**
 * Whether a server that names a token variable got its token.
 */
export type McpAuthState = "token" | "auth_failed" | "anonymous";

/**
 * A server's health as of the last contact attempt.
 */
export interface McpServerHealth {
  "auth"?: McpAuth | null;
  "checked_at"?: string | null;
  "error"?: string | null;
  "name": string;
  "state": McpServerState;
  "tools"?: Array<McpToolInfo>;
  "transport": string;
}

/**
 * Whether a configured server is usable right now.
 */
export type McpServerState = "unknown" | "ready" | "failed";

/**
 * One tool a server offers, as Aether will expose it.
 */
export interface McpToolInfo {
  "declared": boolean;
  "name": string;
  "qualified_name": string;
  "side_effect": SideEffectClass;
}

/**
 * Which memories each model call of a task, or one invocation, was given,
 * oldest snapshot first. `truncated` says snapshots past the limit were left out.
 */
export interface MemoriesInContext {
  "snapshots": Array<SnapshotMemories>;
  "truncated"?: boolean;
}

/**
 * A proposed memory. Admission decides whether it becomes a record.
 */
export interface MemoryCandidate {
  "confidence"?: number;
  "importance"?: number;
  "kind": MemoryKind;
  "provenance"?: MemoryProvenance;
  "scope": MemoryScope;
  "structured"?: unknown;
  "supersedes"?: MemoryId | null;
  "tags"?: Array<string>;
  "text": string;
}

export interface MemoryConsolidation {
  "clusters": Array<ConsolidatedCluster>;
  "dry_run": boolean;
  "scanned": number;
}

export interface MemoryCounts {
  "admitted": number;
  "candidates": number;
  "held_out": number;
  "implicit": number;
  "stale": number;
  "total": number;
}

export type MemoryEvidence = "operator" | "review" | "outcome";

export interface MemoryExport {
  "artifact": string;
  "digest": string;
  "records": number;
}

export interface MemoryFeedback {
  "actor": Actor;
  "created_at": string;
  "evidence": MemoryEvidence;
  "id": MemoryFeedbackId;
  "invocation"?: InvocationId | null;
  "memory": MemoryId;
  "reason": string;
  "retracted_at"?: string | null;
  "retraction_reason"?: string | null;
  "task": TaskId;
  "verdict": MemoryVerdict;
}

/**
 * A mfb_ prefixed ULID.
 * Pattern: ^mfb_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type MemoryFeedbackId = string;

export interface MemoryFeedbackList {
  "ranking"?: FeedbackRanking;
  "records": Array<MemoryFeedback>;
  "truncated": boolean;
}

/**
 * A mem_ prefixed ULID.
 * Pattern: ^mem_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type MemoryId = string;

export interface MemoryImport {
  "conflicts"?: Array<ImportConflict>;
  "digest": string;
  "dry_run": boolean;
  "inserted": number;
  "records": number;
  "unchanged": number;
}

export type MemoryKind = "fact" | "decision" | "lesson" | "constraint" | "preference";

export interface MemoryMigration {
  "applied": boolean;
  "name": string;
  "version": number;
}

export interface MemoryMigrationStatus {
  "backups"?: Array<MigrationBackup>;
  "last_import"?: LastImport | null;
  "memories": MemoryCounts;
  "migrations": Array<MemoryMigration>;
  "schema_version": number;
}

export interface MemoryProvenance {
  "actor"?: Actor | null;
  "agent"?: AgentId | null;
  "artifact"?: ArtifactId | null;
  "event"?: EventId | null;
  "source"?: MemorySource | null;
  "task"?: TaskId | null;
}

export interface MemoryQuerySpec {
  "id"?: MemoryId | null;
  "include_forgotten"?: boolean;
  "include_superseded"?: boolean;
  "kinds"?: Array<MemoryKind>;
  "limit"?: number;
  "proposed"?: boolean;
  "provenance_agent"?: AgentId | null;
  "recorded_by"?: MemoryRecorder | null;
  "scope"?: MemoryScope | null;
  "tags"?: Array<string>;
  "text"?: string | null;
}

/**
 * A stored memory.
 */
export interface MemoryRecord {
  "accepted_at"?: string | null;
  "confidence": number;
  "created_at": string;
  "forgotten_at"?: string | null;
  "id": MemoryId;
  "importance": number;
  "kind": MemoryKind;
  "last_retrieved_at"?: string | null;
  "pinned"?: boolean;
  "proposed_at"?: string | null;
  "provenance": MemoryProvenance;
  "retracted"?: Retraction | null;
  "retrieval_count"?: number;
  "scope": MemoryScope;
  "standing"?: MemoryStanding | null;
  "structured"?: unknown;
  "superseded_by"?: MemoryId | null;
  "supersedes"?: MemoryId | null;
  "tags"?: Array<string>;
  "text": string;
}

/**
 * Who recorded a memory, as a query names it: read from the runtime-set
 * `provenance.actor`, never from anything an author supplied.
 */
export type MemoryRecorder = "operator";

export interface MemoryRef {
  "kind": RefKind;
  "value": string;
}

/**
 * Tagged by `scope`.
 */
export interface MemoryScopeGlobal {
  "scope": "global";
}

export interface MemoryScopeRepository {
  "id": string;
  "scope": "repository";
}

export interface MemoryScopeRole {
  "role": string;
  "scope": "role";
}

export interface MemoryScopeAgent {
  "id": AgentId;
  "scope": "agent";
}

/**
 * A repository's notes for one role family: agents in repository `id`
 * whose role is `role` or extends it at a dot (see [`role_families`]).
 */
export interface MemoryScopeRepositoryRole {
  "id": string;
  "role": string;
  "scope": "repository_role";
}

/**
 * Recorded by `root` for itself and every agent beneath it.
 */
export interface MemoryScopeSubtree {
  "root": AgentId;
  "scope": "subtree";
}

export type MemoryScope =
  | MemoryScopeGlobal
  | MemoryScopeRepository
  | MemoryScopeRole
  | MemoryScopeAgent
  | MemoryScopeRepositoryRole
  | MemoryScopeSubtree;

/**
 * A committed file a memory was read from, and the commit it was read at.
 * The commit is absent when the tree is not a git repository, or git is not
 * on the machine: the note still stands, its version is just unknown.
 */
export interface MemorySource {
  "commit"?: string | null;
  "file": string;
}

/**
 * What admission and retrieval know about a record, sent beside it to a
 * client that lists `memory_v2`. Never part of the stored body.
 */
export interface MemoryStanding {
  "refs"?: Array<MemoryRef>;
  "stale"?: StaleNote | null;
  "state": MemoryState;
  "support": MemorySupport;
  "tier": MemoryTier;
  "verification": Verification;
}

/**
 * Where a memory stands. The store keeps only `Candidate` or `Active`:
 * superseded and retracted are read from the record, which already says so.
 */
export type MemoryState = "candidate" | "active" | "superseded" | "retracted";

/**
 * Distinct supporting tasks, and their distinct runs: the independent ones.
 */
export interface MemorySupport {
  "independent": number;
  "tasks": number;
}

export type MemoryTier = "working" | "operational" | "architecture" | "history" | "evidence";

export type MemoryVerdict = "helped" | "hurt" | "neutral";

/**
 * One turn of conversation as the model sees it.
 */
export interface Message {
  "content": Array<ContentBlock>;
  "role": MessageRole;
}

/**
 * One agent's copy of a fanned-out message.
 */
export interface MessageDelivery {
  "agent": AgentId;
  "message": MessageId;
}

/**
 * A msg_ prefixed ULID.
 * Pattern: ^msg_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type MessageId = string;

export type MessageRole = "user" | "assistant";

export interface MigrationBackup {
  "at": string;
  "bytes": number;
  "from_version": number;
  "name": string;
  "to_version": number;
}

/**
 * One line of the run's story: the events a person would want to see.
 */
export interface Milestone {
  "agent"?: AgentId | null;
  "at": string;
  "detail": string;
  "kind": string;
}

export type MissClass = "cue" | "discrimination" | "execution" | "careless" | "structural";

/**
 * When several occurrences elapsed between scheduler ticks, coalescing
 * delivers one trigger; skipping advances the clock without delivering one.
 * A tick that is late by less than one recurrence still delivers normally.
 */
export type MissedRunPolicy = "coalesce" | "skip";

/**
 * Price per million tokens in USD.
 */
export interface ModelCost {
  "above_threshold"?: ModelCostTier | null;
  "cache_read_per_mtok"?: number;
  "cache_write_1h_per_mtok"?: number | null;
  "cache_write_per_mtok"?: number;
  "input_per_mtok": number;
  "output_per_mtok": number;
}

/**
 * Rates for an entire request whose prompt exceeds `prompt_tokens`.
 */
export interface ModelCostTier {
  "cache_read_per_mtok"?: number;
  "cache_write_1h_per_mtok"?: number | null;
  "cache_write_per_mtok"?: number;
  "input_per_mtok": number;
  "output_per_mtok": number;
  "prompt_tokens": number;
}

/**
 * Normalized failure class. Every adapter maps its provider's errors here.
 */
export type ModelErrorClass = "rate_limit" | "overloaded" | "context_limit" | "invalid_request" | "not_found" | "network" | "timeout" | "cancelled" | "server" | "unknown" | "auth" | "permission" | "quota_exhausted" | "usage_limit";

export interface ModelInfo {
  "aliases"?: Array<string>;
  "auth_source"?: string | null;
  "capabilities": CapabilityProfile;
  "configured": boolean;
  "is_default": boolean;
  "model": ModelRef;
  "name": string;
  "provider_name": string;
  "tier"?: string | null;
}

/**
 * A provider-qualified model name, rendered as `provider/model`.
 */
export interface ModelRef {
  "model": string;
  "provider": string;
}

/**
 * Unset means the first ready subscription model.
 */
export interface ModelSettings {
  "cheap"?: string | null;
  "strong"?: string | null;
}

export interface ModelsPatch {
  "cheap"?: string | null;
  "strong"?: string | null;
}

/**
 * What a mount forwards to.
 */
export type MountKind = "archon";

export type NetworkPolicy = "denied" | "allowed";

/**
 * A remembered decision written directly, without answering a call first.
 */
export interface NewApprovalRule {
  "decision": ApprovalDecision;
  "idempotency_key"?: string | null;
  "match": RuleMatch;
  "note"?: string | null;
  "scope": RuleScope;
}

export type NoesisBilling = "subscription" | "api";

/**
 * The strong slot resolved: `model` when one would run, else `refusal`.
 */
export interface NoesisModelChoice {
  "model"?: string | null;
  "refusal"?: string | null;
}

/**
 * A model as Noesis sees it: how it is paid for and whether a step on it
 * could run now (an engine detected and signed in, an API provider keyed).
 */
export interface NoesisModelOffer {
  "billing": NoesisBilling;
  "id": string;
  "ready": boolean;
}

/**
 * Where a proposition or chain came from.
 */
export interface NoesisProvenance {
  "message_ref"?: string | null;
  "source": string;
}

export interface NoesisSession {
  "active_problem"?: Problem | null;
  "agent": AgentId;
  "created_at": string;
  "goal": string;
  "id": string;
  "mode": SessionMode;
  "open_question"?: Question | null;
  "phase": Phase;
  "plan"?: PlanTree | null;
  "updated_at": string;
}

/**
 * A subset of the `[noesis]` table to write. Absent keys keep their value.
 */
export interface NoesisSettingsPatch {
  "billing"?: BillingPatch | null;
  "budget"?: BudgetPatch | null;
  "concurrency"?: ConcurrencyPatch | null;
  "disable"?: DisablePatch | null;
  "enabled"?: boolean | null;
  "engineering_context"?: boolean | null;
  "extraction"?: ExtractionPatch | null;
  "models"?: ModelsPatch | null;
  "priority"?: PriorityPatch | null;
  "prompts"?: PromptsPatch | null;
}

/**
 * The `[noesis]` config table (§6.1) as one JSON object of the same shape.
 */
export interface NoesisSettingsView {
  "billing": BillingSettings;
  "budget": BudgetSettings;
  "concurrency": ConcurrencySettings;
  "disable": DisableSettings;
  "enabled": boolean;
  "engineering_context": boolean;
  "extraction": ExtractionSettings;
  "models": ModelSettings;
  "priority": PrioritySettings;
  "prompts": PromptSettings;
}

/**
 * Reply to `noesis_status` and `noesis_configure`.
 */
export interface NoesisStatus {
  "disable_policy": DisablePolicy;
  "effective": NoesisSettingsView;
  "enabled": boolean;
  "jobs_running": number;
  "models"?: Array<NoesisModelOffer>;
  "prompt_overrides": Array<string>;
  "settings": NoesisSettingsView;
  "spend_today": SpendToday;
  "teaching_model"?: NoesisModelChoice | null;
}

/**
 * Whether the agent's model is shown the note as well as people.
 */
export type NoteContext = "none" | "digest";

/**
 * How a note reads on screen: a plain remark, or the result of something a
 * host did for the operator. It never reaches the model on its own.
 */
export type NoteLevel = "info" | "applied" | "refused";

export interface Notification {
  "actions"?: Array<NotificationAction>;
  "answer"?: NotificationAnswer | null;
  "body"?: string;
  "created_at": string;
  "free_text"?: boolean;
  "id": NotificationId;
  "key"?: string | null;
  "kind": NotificationKind;
  "link"?: string | null;
  "source": string;
  "state": NotificationState;
  "title": string;
  "updated_at": string;
}

/**
 * A choice the operator can make; its `id` is what the poster receives.
 */
export interface NotificationAction {
  "id": string;
  "label": string;
}

/**
 * The operator's answer: a chosen action, their own words, or both.
 */
export interface NotificationAnswer {
  "action"?: string | null;
  "at": string;
  "by": Actor;
  "text"?: string | null;
}

/**
 * A ntf_ prefixed ULID.
 * Pattern: ^ntf_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type NotificationId = string;

/**
 * What a notification asks of the operator. Questions and blockers wait on
 * them; the rest inform.
 */
export type NotificationKind = "question" | "blocker" | "decision" | "outcome" | "info";

/**
 * What a poster sends. Posting the same `key` again from the same source
 * updates that notification instead of adding another.
 */
export interface NotificationPost {
  "actions"?: Array<NotificationAction>;
  "body"?: string;
  "free_text"?: boolean;
  "key"?: string | null;
  "kind": NotificationKind;
  "link"?: string | null;
  "title": string;
}

export type NotificationState = "unread" | "read" | "resolved";

/**
 * Something the run left unfinished.
 * Tagged by `open`.
 */
export interface OpenLoopAgent {
  "id": AgentId;
  "name": string;
  "open": "agent";
}

export interface OpenLoopTask {
  "id": TaskId;
  "objective": string;
  "open": "task";
}

export interface OpenLoopApproval {
  "description": string;
  "id": ApprovalId;
  "open": "approval";
  "tool": string;
}

export type OpenLoop = OpenLoopAgent | OpenLoopTask | OpenLoopApproval;

/**
 * Operator applicability, not authorship. Repository identity is resolved by the runtime.
 * Tagged by `scope`.
 */
export interface OperatorMemoryScopeAgent {
  "id": AgentId;
  "scope": "agent";
}

export interface OperatorMemoryScopeGlobal {
  "scope": "global";
}

export interface OperatorMemoryScopeRepository {
  "repository"?: string | null;
  "root"?: string | null;
  "scope": "repository";
}

export interface OperatorMemoryScopeRole {
  "role": string;
  "scope": "role";
}

export interface OperatorMemoryScopeRepositoryRole {
  "repository"?: string | null;
  "role": string;
  "root"?: string | null;
  "scope": "repository_role";
}

export type OperatorMemoryScope =
  | OperatorMemoryScopeAgent
  | OperatorMemoryScopeGlobal
  | OperatorMemoryScopeRepository
  | OperatorMemoryScopeRole
  | OperatorMemoryScopeRepositoryRole;

/**
 * A person supplies content and applicability, never fabricated provenance.
 */
export interface OperatorMemorySpec {
  "confidence"?: number | null;
  "importance"?: number | null;
  "kind": MemoryKind;
  "proposed"?: boolean;
  "scope": OperatorMemoryScope;
  "supersedes"?: MemoryId | null;
  "tags"?: Array<string>;
  "text": string;
}

/**
 * Server-observed connection identity, never a bearer token or client-supplied name.
 * Tagged by `transport`.
 */
export interface OperatorOriginLocalSocket {
  "connection": string;
  "transport": "local_socket";
}

export interface OperatorOriginTcp {
  "connection": string;
  "peer": string;
  "transport": "tcp";
}

/**
 * A paired remote device, named as the daemon enrolled it.
 */
export interface OperatorOriginDevice {
  "connection": string;
  "device": DeviceId;
  "name": string;
  "transport": "device";
}

export type OperatorOrigin = OperatorOriginLocalSocket | OperatorOriginTcp | OperatorOriginDevice;

/**
 * Body of `POST /api/pair/claim`.
 */
export interface PairClaimRequest {
  "code": string;
  "device_name": string;
  "public_key": string;
}

export interface PairClaimResponse {
  "expires_at": string;
  "fingerprint": string;
  "pairing": string;
  "phrase": string;
}

export interface PairStatusRequest {
  "pairing": string;
}

export interface PairStatusResponse {
  "device"?: DeviceIdentity | null;
  "state": PairingState;
}

/**
 * One pairing attempt. Reply to the four pairing commands.
 */
export interface Pairing {
  "candidate"?: PairingCandidate | null;
  "code"?: string | null;
  "device"?: DeviceIdentity | null;
  "expires_at": string;
  "id": string;
  "role": DeviceRole;
  "state": PairingState;
  "url"?: string | null;
}

/**
 * The device asking to be enrolled, as the host must confirm it.
 */
export interface PairingCandidate {
  "claimed_at": string;
  "fingerprint": string;
  "name": string;
  "phrase": string;
}

export type PairingState = "enrolled" | "rejected" | "expired" | "cancelled" | "open" | "claimed";

export interface PathCompletion {
  "is_dir": boolean;
  "path": string;
}

export type PathCompletionKind = "directory" | "workspace_file";

export interface PathCompletions {
  "entries": Array<PathCompletion>;
  "truncated": boolean;
}

export interface PendingArtifactCleanup {
  "pending_unlinks": Array<ArtifactId>;
  "staged_publications": Array<ArtifactId>;
}

/**
 * An agent's question nobody has answered or superseded.
 */
export interface PendingQuestion {
  "agent": AgentId;
  "agent_name"?: string | null;
  "asked_at": string;
  "question": string;
  "question_seq": number;
}

export type Phase = "probe" | "plan" | "teach" | "practice";

export interface PlanEdge {
  "from": string;
  "to": string;
}

export interface PlanNode {
  "id": string;
  "status": PlanNodeStatus;
  "title": string;
}

export type PlanNodeStatus = "upcoming" | "current" | "visited";

export interface PlanTree {
  "edges": Array<PlanEdge>;
  "nodes": Array<PlanNode>;
  "revision": number;
}

export type PracticeOutcome = "solved" | "solved_with_hints" | "moved_on" | "surrendered" | "dismissed";

export type Priority = "low" | "normal" | "high" | "critical";

export interface PriorityPatch {
  "background"?: BackgroundPriority | null;
}

export interface PrioritySettings {
  "background": BackgroundPriority;
}

/**
 * A practice problem. Its solution is revealed only by
 * `noesis.practice_concluded`.
 */
export interface Problem {
  "id": string;
  "mode": ProblemMode;
  "posed_at": string;
  "statement": string;
  "targets": Array<string>;
}

export type ProblemMode = "recall" | "discrimination" | "composition";

export interface PromptSettings {
  "overrides": boolean;
}

export interface PromptsPatch {
  "overrides"?: boolean | null;
}

export interface Proposition {
  "aliases": Array<string>;
  "created_at": string;
  "id": string;
  "name": string;
  "provenance": Array<NoesisProvenance>;
  "statement": string;
}

/**
 * Where an imported run came from. Absent on a run this daemon ran itself.
 */
export interface Provenance {
  "exported_at": string;
  "source_instance": string;
  "source_run": RunId;
}

/**
 * Why a provider is being avoided, and until when.
 */
export interface ProviderBackoff {
  "reason": string;
  "until": string;
}

/**
 * Read-time projection of the latest report; unknown usage is always stale.
 */
export interface ProviderHeadroom {
  "age_seconds"?: number | null;
  "observed_at"?: string | null;
  "stale": boolean;
  "windows": Array<WindowHeadroom>;
}

/**
 * One provider's scheduling picture. Derived state: the scheduler rebuilds
 * it from the turns in flight and the feedback providers have given.
 */
export interface ProviderHealth {
  "backoff"?: ProviderBackoff | null;
  "last_fallback"?: FallbackRecord | null;
  "limit"?: number | null;
  "provider": string;
  "quota"?: QuotaWindow | null;
  "recent_errors"?: Array<ErrorTally>;
  "running": number;
  "suspended_agents"?: number;
  "usage"?: ProviderUsage | null;
}

/**
 * One provider as a client sees it: what it is, whether it can be used, and
 * how it is behaving right now. `health` is live scheduler state, so a
 * provider that has done nothing since the daemon started reads as idle.
 */
export interface ProviderInfo {
  "auth_source"?: string | null;
  "base_url"?: string;
  "configured": boolean;
  "enabled": boolean;
  "engine": boolean;
  "family"?: string | null;
  "headroom"?: ProviderHeadroom;
  "health": ProviderHealth;
  "models": number;
  "name": string;
  "open_model_list": boolean;
  "protocol": string;
  "provider": string;
}

/**
 * Latest normalized account report. The observation time is not a reset time.
 */
export interface ProviderUsage {
  "observed_at": string;
  "windows": Array<UsageWindow>;
}

/**
 * Publish UTF-8 evidence associated with a durable task. The daemon owns
 * blob storage and derives the agent/run association from that task.
 */
export interface PublishArtifact {
  "content_type": string;
  "name": string;
  "summary": string;
  "task": TaskId;
  "text": string;
}

/**
 * Reply to the push commands: what the daemon sends this device.
 */
export interface PushConfig {
  "enabled": boolean;
  "kinds"?: Array<AttentionKind>;
  "previews"?: boolean;
  "subscribed"?: boolean;
  "vapid_public_key"?: string | null;
}

/**
 * The decrypted body of one Web Push message.
 */
export interface PushPayload {
  "agent"?: AgentId | null;
  "agent_name"?: string | null;
  "approval"?: ApprovalId | null;
  "at": string;
  "instance_id": string;
  "kind": AttentionKind;
  "preview"?: string | null;
  "question_seq"?: number | null;
  "seq": number;
  "task"?: TaskId | null;
  "title": string;
  "url": string;
  "v": number;
}

/**
 * A browser's `PushSubscription`, as `toJSON()` gives it.
 */
export interface PushSubscription {
  "auth": string;
  "endpoint": string;
  "p256dh": string;
}

/**
 * A question put to the learner. The answer key is never part of it: it
 * is revealed in a [`QuestionResult`] once the answer is recorded.
 */
export interface Question {
  "collapsed": boolean;
  "concepts": Array<string>;
  "id": string;
  "kind": QuestionKind;
  "multi": boolean;
  "options"?: Array<QuestionOption> | null;
  "plan_node"?: string | null;
  "posed_at": string;
  "prompt": string;
  "step"?: QuestionStep | null;
}

export type QuestionKind = "choice" | "recall" | "goal";

export interface QuestionOption {
  "label": string;
  "value": string;
}

export interface QuestionResult {
  "answer_key"?: AnswerKey | null;
  "correct"?: boolean | null;
  "explanation"?: string | null;
  "question": string;
}

/**
 * What a question does in the lesson: a probe measures, a teach-phase
 * question predicts before a step or checks after it.
 */
export type QuestionStep = "measure" | "predict" | "check" | "goal" | "practice";

/**
 * A subscription usage window as an engine reported it. Opaque JSON on the
 * wire from the CLI; normalized here so every reader agrees.
 */
export interface QuotaWindow {
  "resets_at"?: string | null;
  "used_percent": number;
  "window_minutes"?: number | null;
}

/**
 * How an interrupted turn should be handled after a restart.
 */
export type RecoveryClass = "resume" | "retry" | "inspect";

/**
 * Tagged by `kind`.
 */
export interface RecurrenceInterval {
  "kind": "interval";
  "seconds": number;
}

/**
 * Five fields: minute, hour, day of month, month, day of week.
 */
export interface RecurrenceCron {
  "expression": string;
  "kind": "cron";
  "timezone": string;
}

export type Recurrence = RecurrenceInterval | RecurrenceCron;

export type RefKind = "repo" | "path" | "symbol" | "commit" | "issue" | "pr" | "test" | "review" | "doc";

/**
 * How to remember the answer a person just gave. The defaults are the
 * narrowest reading of it: the answered agent, this tool, and what this
 * call touched. Anything wider is asked for explicitly.
 */
export interface RememberSpec {
  "note"?: string | null;
  "scope"?: RuleScope | null;
  "tool_wide"?: boolean;
}

/**
 * One prefix of the listener forwarded to a loopback target.
 */
export interface RemoteMount {
  "kind": MountKind;
  "prefix": string;
  "target": string;
}

/**
 * The runtime-changeable remote settings, saved whole under a revision.
 */
export interface RemoteSettings {
  "enabled"?: boolean;
  "mounts"?: Array<RemoteMount> | null;
  "pairing"?: boolean;
  "public_origins"?: Array<string>;
  "push"?: boolean;
}

/**
 * Reply to `get_remote` and `configure_remote`.
 */
export interface RemoteStatus {
  "address": string;
  "allow_remote"?: boolean;
  "connections"?: number;
  "devices"?: number;
  "error"?: string | null;
  "listening"?: string | null;
  "push_previews"?: boolean;
  "revision": number;
  "settings": RemoteSettings;
}

export interface ReplayApproval {
  "decided_by"?: Actor | null;
  "decision"?: ApprovalDecision | null;
  "description": string;
  "id": ApprovalId;
  "requested_at": string;
  "tool": string;
}

/**
 * The shape of what the model was given. The layer texts are here; the
 * whole snapshot is a separate record, named so a caller can fetch it.
 */
export interface ReplayContext {
  "layers": Array<ContextLayer>;
  "model": ModelRef;
  "snapshot": SnapshotId;
  "token_estimate": number;
}

/**
 * One turn of one agent.
 */
export interface ReplayFrame {
  "agent": AgentId;
  "approvals": Array<ReplayApproval>;
  "context"?: ReplayContext | null;
  "duration_ms": number;
  "ended_at"?: string | null;
  "from_seq": number;
  "messages": Array<ReplayMessage>;
  "notes"?: Array<ReplayNote>;
  "outcome": TurnOutcome;
  "started_at": string;
  "to_seq": number;
  "tools": Array<ReplayTool>;
  "turn": number;
  "usage": Usage;
}

export interface ReplayMessage {
  "at": string;
  "content": Array<ContentBlock>;
  "invocation"?: InvocationId | null;
  "role": MessageRole;
}

/**
 * A note appended to the transcript; it started no turn.
 */
export interface ReplayNote {
  "at": string;
  "context"?: NoteContext;
  "level"?: NoteLevel;
  "links"?: Array<string>;
  "seq": number;
  "text": string;
}

export interface ReplayTool {
  "call": ToolCallId;
  "input": unknown;
  "name": string;
  "outcome": ToolResult;
  "requested_at": string;
  "side_effect": SideEffectClass;
}

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

/**
 * Reply to `list_agents_page`. `next_after` is the cursor for the next
 * page, absent on the last one. Ids are only ordered, not committed in
 * order, so a walk covers the agents present when it began; a client
 * subscribes first and applies the buffered events for creations during it.
 */
export interface ReplyAgentsPage {
  "data": Record<string, unknown>;
  "reply": "agents_page";
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

export interface ReplyNotification {
  "data": Notification;
  "reply": "notification";
}

/**
 * Newest first.
 */
export interface ReplyNotifications {
  "data": Array<Notification>;
  "reply": "notifications";
}

/**
 * The answered notification, and the posting agent whose inbox the
 * answer reached; `None` for a service, which reads the event.
 */
export interface ReplyNotificationAnswered {
  "data": Record<string, unknown>;
  "reply": "notification_answered";
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

/**
 * Reply to `get_transcript_page`: entries in ascending `seq`, whether
 * older or newer ones exist outside it, and the seq of the agent's newest
 * entry (0 for none).
 */
export interface ReplyTranscriptPage {
  "data": Record<string, unknown>;
  "reply": "transcript_page";
}

export interface ReplyUsageBreakdown {
  "data": UsageBreakdown;
  "reply": "usage_breakdown";
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

export interface ReplyMemoryRetrievalExplain {
  "data": RetrievalExplain;
  "reply": "memory_retrieval_explain";
}

export interface ReplyMemoryConsolidation {
  "data": MemoryConsolidation;
  "reply": "memory_consolidation";
}

export interface ReplyMemoryExport {
  "data": MemoryExport;
  "reply": "memory_export";
}

export interface ReplyMemoryImport {
  "data": MemoryImport;
  "reply": "memory_import";
}

export interface ReplyMemoryMigrationStatus {
  "data": MemoryMigrationStatus;
  "reply": "memory_migration_status";
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

export interface ReplyViewModes {
  "data": ViewModes;
  "reply": "view_modes";
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

export interface ReplyNoesisStatus {
  "data": NoesisStatus;
  "reply": "noesis_status";
}

export interface ReplyNoesisSession {
  "data": NoesisSession;
  "reply": "noesis_session";
}

/**
 * `next_after` is the cursor for the next page, absent on the last one.
 */
export interface ReplyNoesisSessions {
  "data": Record<string, unknown>;
  "reply": "noesis_sessions";
}

/**
 * `truncated` is set when `limit` cut the graph short.
 */
export interface ReplyNoesisGraph {
  "data": Record<string, unknown>;
  "reply": "noesis_graph";
}

export interface ReplyNoesisJob {
  "data": Record<string, unknown>;
  "reply": "noesis_job";
}

export interface ReplyNoesisImport {
  "data": Record<string, unknown>;
  "reply": "noesis_import";
}

export interface ReplyNoesisJobs {
  "data": Record<string, unknown>;
  "reply": "noesis_jobs";
}

export interface ReplyNoesisLearner {
  "data": Record<string, unknown>;
  "reply": "noesis_learner";
}

/**
 * `next_before` is the cursor for the next page, absent on the last one.
 */
export interface ReplyNoesisEvidence {
  "data": Record<string, unknown>;
  "reply": "noesis_evidence";
}

export interface ReplyConceptState {
  "data": ConceptState;
  "reply": "concept_state";
}

export type Reply =
  | ReplyStorageReport
  | ReplyRetentionPreview
  | ReplyGithubWatcher
  | ReplySchedule
  | ReplySchedules
  | ReplyScheduleHistory
  | ReplyWelcome
  | ReplyStatus
  | ReplyOk
  | ReplyInterrupted
  | ReplyPaused
  | ReplyAgent
  | ReplyAgents
  | ReplyAgentsPage
  | ReplyTree
  | ReplyTask
  | ReplyTasks
  | ReplyMessageSent
  | ReplyNoteAppended
  | ReplyNotification
  | ReplyNotifications
  | ReplyNotificationAnswered
  | ReplyMessagesSent
  | ReplyApprovals
  | ReplyApprovalRule
  | ReplyApprovalRules
  | ReplySubscribed
  | ReplyEvents
  | ReplyTranscript
  | ReplyTranscriptPage
  | ReplyUsageBreakdown
  | ReplyInbox
  | ReplySnapshot
  | ReplyInvocation
  | ReplyInvocations
  | ReplyCheckpoint
  | ReplyArtifact
  | ReplyArtifacts
  | ReplyArtifactBytes
  | ReplyMemories
  | ReplyRepositoryAliased
  | ReplyRepositoryAliases
  | ReplyMemoriesInContext
  | ReplyMemory
  | ReplyMemoryFeedback
  | ReplyMemoryFeedbackList
  | ReplyMemoryRetrievalExplain
  | ReplyMemoryConsolidation
  | ReplyMemoryExport
  | ReplyMemoryImport
  | ReplyMemoryMigrationStatus
  | ReplyMemorySeeded
  | ReplyRuns
  | ReplyRunReport
  | ReplyReplay
  | ReplyRunExported
  | ReplyRunImported
  | ReplyMcpServers
  | ReplyModels
  | ReplyViewModes
  | ReplyProviders
  | ReplyProviderUsage
  | ReplyAuth
  | ReplyEngineTool
  | ReplyEngineTools
  | ReplyAttention
  | ReplyPathCompletions
  | ReplyWorkspaces
  | ReplyWorkspaceValidation
  | ReplyGitSummary
  | ReplyWorkspaceDiff
  | ReplyWorkspaceIntegration
  | ReplyWorkspaceDiscarded
  | ReplyRemote
  | ReplyPairing
  | ReplyDevice
  | ReplyDevices
  | ReplyPushConfig
  | ReplySubmissionResolved
  | ReplyNoesisStatus
  | ReplyNoesisSession
  | ReplyNoesisSessions
  | ReplyNoesisGraph
  | ReplyNoesisJob
  | ReplyNoesisImport
  | ReplyNoesisJobs
  | ReplyNoesisLearner
  | ReplyNoesisEvidence
  | ReplyConceptState;

/**
 * An old repository name memory used to be filed under, and the id it now
 * resolves to. Registered by an operator; never guessed.
 */
export interface RepositoryAlias {
  "alias": string;
  "canonical": string;
  "created_at": string;
}

/**
 * What registering an alias moves, or moved: every memory under the alias,
 * in any state, and the agents whose stored scope still names it.
 */
export interface RepositoryAliasCounts {
  "agents": number;
  "duplicates": number;
  "repository": number;
  "repository_role": number;
}

/**
 * The answer to registering an alias, or to a dry run of it.
 */
export interface RepositoryAliasReport {
  "alias": string;
  "canonical": string;
  "counts": RepositoryAliasCounts;
  "dry_run": boolean;
  "existing": boolean;
}

/**
 * Resource keys one agent's mutating tool call holds for the length of that
 * call. Two agents never hold conflicting keys at once (VISION §3.6 limit 5).
 */
export interface ResourceHolding {
  "agent": AgentId;
  "keys": Array<ResourceKey>;
  "quarantined"?: boolean;
  "since": string;
}

/**
 * A resource a call touches, used to decide which calls may run together.
 * Conventional forms: `file:<absolute path>`, `dir:<absolute path>`,
 * `repo:<root>`, `network`, `agent:<id>`.
 */
export type ResourceKey = string;

export type Result_of_Reply_or_ErrorReply = Record<string, unknown>;

export interface RetentionPreview {
  "artifact_payload_bytes": number;
  "at": string;
  "eligible_artifacts": Array<ArtifactId>;
  "history_before": string;
  "history_payload_bytes": number;
  "pending_cleanup": PendingArtifactCleanup;
  "runs": Array<RunRetentionPreview>;
}

/**
 * Why a repository-knowledge sweep curated a record out: the committed tree
 * named here no longer states it, so the `file@sha` label it carried is no
 * longer a promise anyone can check.
 *
 * Distinct from a person's `forget` on purpose. A person's curation outranks
 * the file however many times the note comes back; a retraction is undone by
 * the note reappearing in the tree.
 */
export interface Retraction {
  "at": string;
  "commit": string;
  "file": string;
}

/**
 * What retrieval did for each model call of a task, or one invocation.
 */
export interface RetrievalExplain {
  "truncated"?: boolean;
  "turns": Array<ExplainedTurn>;
}

/**
 * What retrieval did for one turn, recorded with the snapshot it built.
 */
export interface RetrievalTrace {
  "candidates": Array<TraceCandidate>;
  "omitted"?: TraceOmitted;
  "policy": TracePolicy;
  "pool": number;
}

/**
 * Why a rule stopped applying. Kept on the record so the log explains a
 * decision that would be made differently today.
 */
export interface Revocation {
  "at": string;
  "by": Actor;
  "reason"?: string | null;
}

/**
 * Opaque role label; an orchestration layer gives it meaning.
 * The harness reads only its dots, to pick a repository's notes for a role
 * family (`memory::role_families`).
 */
export type Role = string;

/**
 * What a rule matches. An absent field matches anything, so the empty
 * matcher covers every call — deliberately, so "allow everything for this
 * run" is expressible without a magic pattern.
 */
export interface RuleMatch {
  "argument"?: string | null;
  "resource_prefix"?: string | null;
  "tool"?: string | null;
}

/**
 * Where a remembered decision applies.
 * Tagged by `scope`.
 */
/**
 * Every agent this daemon runs.
 */
export interface RuleScopeEverywhere {
  "scope": "everywhere";
}

/**
 * One agent only.
 */
export interface RuleScopeAgent {
  "id": AgentId;
  "scope": "agent";
}

/**
 * An agent and everything it delegates to.
 */
export interface RuleScopeSubtree {
  "root": AgentId;
  "scope": "subtree";
}

/**
 * Every agent in one run.
 */
export interface RuleScopeRun {
  "id": RunId;
  "scope": "run";
}

export type RuleScope = RuleScopeEverywhere | RuleScopeAgent | RuleScopeSubtree | RuleScopeRun;

/**
 * Everything derived from the run's own work. Carries no local sequence
 * numbers, so two stores holding the same run produce equal bodies once
 * their ids are put side by side ([`RunReport::canonical_json`]).
 */
export interface RunBody {
  "agents": Array<AgentLine>;
  "approvals": Array<ApprovalLine>;
  "artifacts": Array<ArtifactLine>;
  "duration_ms": number;
  "ended_at"?: string | null;
  "events": number;
  "failures": Array<FailureLine>;
  "objective"?: string | null;
  "open_loops": Array<OpenLoop>;
  "outcome": RunOutcome;
  "started_at": string;
  "tasks": Array<TaskLine>;
  "timeline": Array<Milestone>;
  "tools": Array<ToolLine>;
  "usage": Usage;
}

/**
 * A run_ prefixed ULID.
 * Pattern: ^run_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type RunId = string;

/**
 * One row of `aether runs list`, projected from a run's agents and tasks.
 */
export interface RunListing {
  "agents": number;
  "agents_imported"?: number;
  "agents_live": number;
  "objective"?: string | null;
  "run": RunId;
  "started_at": string;
  "updated_at": string;
  "usage": Usage;
}

/**
 * A run's state, read from its agents rather than declared anywhere.
 */
export type RunOutcome = "completed" | "failed" | "cancelled" | "running";

/**
 * A run's report: a header that says which run and when, and a body that
 * says what happened. Only the body is comparable across stores - the
 * header carries the local identity.
 */
export interface RunReport {
  "body": RunBody;
  "from_seq": number;
  "generated_at": string;
  "provenance"?: Provenance | null;
  "run": RunId;
  "schema": number;
  "to_seq": number;
}

/**
 * Tagged by `status`.
 */
export interface RunRetentionDecisionEligible {
  "status": "eligible";
}

/**
 * The first blocker, in the same order as the deletion path.
 */
export interface RunRetentionDecisionProtected {
  "reason": string;
  "status": "protected";
}

export type RunRetentionDecision = RunRetentionDecisionEligible | RunRetentionDecisionProtected;

export interface RunRetentionPreview {
  "decision": RunRetentionDecision;
  "events": number;
  "history_payload_bytes": number;
  "rows": number;
  "run": RunId;
}

/**
 * What one run has spent and where that rate leads.
 */
export interface RunSummary {
  "agents": number;
  "agents_live": number;
  "forecast_usd_next_hour": number;
  "rate_usd_per_hour": number;
  "run": RunId;
  "started_at": string;
  "usage": Usage;
}

/**
 * Timers enqueue work, never authority: targets keep their scope and budgets;
 * spawned runs get only what an operator could grant via `create_agent`.
 */
export interface Schedule {
  "busy"?: BusyPolicy;
  "created_at": string;
  "deliveries": number;
  "id": ScheduleId;
  "last"?: ScheduleOccurrence | null;
  "last_inbox_seq"?: number | null;
  "last_spawned"?: AgentId | null;
  "max_deliveries"?: number | null;
  "missed": MissedRunPolicy;
  "name": string;
  "owner": Actor;
  "payload": unknown;
  "recurrence": Recurrence;
  "run"?: RunId | null;
  "spawn"?: SpawnSpec | null;
  "status": ScheduleStatus;
  "target"?: AgentId | null;
  "updated_at": string;
}

export type ScheduleAction = "pause" | "resume" | "cancel";

/**
 * A sch_ prefixed ULID.
 * Pattern: ^sch_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type ScheduleId = string;

/**
 * History describes delivery or spawning, never claims that a model
 * completed the requested work. The ids link to the execution trace.
 */
export interface ScheduleOccurrence {
  "due": string;
  "outcome": ScheduleOutcome;
  "recorded_at": string;
  "schedule": ScheduleId;
}

/**
 * Tagged by `outcome`.
 */
export interface ScheduleOutcomeDelivered {
  "inbox": InboxItemId;
  "outcome": "delivered";
}

/**
 * A spawn schedule started a run: its root agent and task.
 */
export interface ScheduleOutcomeSpawned {
  "agent": AgentId;
  "outcome": "spawned";
  "run": RunId;
  "task": TaskId;
}

export interface ScheduleOutcomeSkipped {
  "outcome": "skipped";
  "reason": string;
}

export interface ScheduleOutcomeFailed {
  "error": string;
  "outcome": "failed";
}

export type ScheduleOutcome =
  | ScheduleOutcomeDelivered
  | ScheduleOutcomeSpawned
  | ScheduleOutcomeSkipped
  | ScheduleOutcomeFailed;

/**
 * Tagged by `state`.
 */
export interface ScheduleStatusActive {
  "next_due": string;
  "state": "active";
}

export interface ScheduleStatusPaused {
  "next_due": string;
  "state": "paused";
}

export interface ScheduleStatusCancelled {
  "at": string;
  "state": "cancelled";
}

export interface ScheduleStatusExhausted {
  "at": string;
  "state": "exhausted";
}

export interface ScheduleStatusFailed {
  "at": string;
  "error": string;
  "state": "failed";
}

export type ScheduleStatus =
  | ScheduleStatusActive
  | ScheduleStatusPaused
  | ScheduleStatusCancelled
  | ScheduleStatusExhausted
  | ScheduleStatusFailed;

/**
 * Execution scope: where an agent may act. A first-class runtime attribute,
 * not prose in a prompt.
 */
export interface Scope {
  "allowed_paths"?: Array<string>;
  "backend"?: ExecutionBackend;
  "labels"?: Record<string, string>;
  "network"?: NetworkPolicy;
  "repository"?: string | null;
  "workspace_root"?: string | null;
}

/**
 * What one pass of reading a repository's committed notes did.
 */
export interface SeedReport {
  "admitted": number;
  "found": number;
  "rejected": number;
  "retracted"?: number;
  "superseded": number;
  "unchanged": number;
}

/**
 * Daemon to client.
 * Tagged by `frame`.
 */
export interface ServerFrameReply {
  "frame": "reply";
  "id": number;
  "result": Result_of_Reply_or_ErrorReply;
}

export interface ServerFrameEvent {
  "event": Event;
  "frame": "event";
  "sub": number;
}

export interface ServerFrameDelta {
  "agent": AgentId;
  "delta": Delta;
  "frame": "delta";
  "invocation": InvocationId;
  "invocation_seq"?: number | null;
  "sub": number;
}

/**
 * The remote carrier speaking first: what a device signs to
 * authenticate its hello. Never sent on the socket or the token listener.
 */
export interface ServerFrameChallenge {
  "frame": "challenge";
  "instance_id": string;
  "nonce": string;
}

export type ServerFrame =
  | ServerFrameReply
  | ServerFrameEvent
  | ServerFrameDelta
  | ServerFrameChallenge;

export type SessionExtraction = "manual" | "auto";

export type SessionMode = "teach" | "practice";

/**
 * What a tool does to the world. Drives retry policy, idempotency tracking
 * and approval requirements.
 */
export type SideEffectClass = "read_only" | "idempotent_mutation" | "mutation" | "external";

/**
 * A snp_ prefixed ULID.
 * Pattern: ^snp_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type SnapshotId = string;

/**
 * The memories one snapshot put in front of the model, as a host asks for
 * them through `memories_in_context`.
 */
export interface SnapshotMemories {
  "agent": AgentId;
  "at": string;
  "invocations"?: Array<InvocationId>;
  "memories"?: Array<MemoryId>;
  "snapshot": SnapshotId;
  "task"?: TaskId | null;
  "turn": number;
}

/**
 * Root agent + task each spawn occurrence creates, in a new run. Fields as on
 * `create_agent`; no parent or existing run.
 */
export interface SpawnSpec {
  "approval"?: ApprovalPolicy | null;
  "config"?: AgentConfig | null;
  "fallbacks"?: Array<ModelRef>;
  "model"?: ModelRef | null;
  "name"?: string | null;
  "objective": string;
  "role": string;
  "scope"?: Scope | null;
}

/**
 * What Noesis spent today, split by who asked for the work.
 */
export interface SpendToday {
  "background": Usage;
  "interactive": Usage;
}

/**
 * Why retrieval demotes a memory as possibly stale, and since when.
 */
export interface StaleNote {
  "reason": string;
  "revision"?: string | null;
  "since": string;
}

/**
 * Why a [`DaemonStatus`] is not fresh, and how old it is.
 */
export interface StatusStale {
  "read_at"?: string | null;
  "waited_ms": number;
}

/**
 * Why the model stopped generating.
 * Tagged by `reason`.
 */
export interface StopReasonEndTurn {
  "reason": "end_turn";
}

export interface StopReasonToolUse {
  "reason": "tool_use";
}

export interface StopReasonMaxTokens {
  "reason": "max_tokens";
}

export interface StopReasonStopSequence {
  "reason": "stop_sequence";
}

export interface StopReasonRefusal {
  "category"?: string | null;
  "reason": "refusal";
}

export interface StopReasonPauseTurn {
  "reason": "pause_turn";
}

export interface StopReasonOther {
  "reason": "other";
  "value": string;
}

export type StopReason =
  | StopReasonEndTurn
  | StopReasonToolUse
  | StopReasonMaxTokens
  | StopReasonStopSequence
  | StopReasonRefusal
  | StopReasonPauseTurn
  | StopReasonOther;

/**
 * What became of a keyed command, as far as the daemon can prove. Each
 * carries the daemon's `horizon` now; `retained_since` is for people to read.
 * Tagged by `outcome`.
 */
/**
 * It ran. `reply` is what a duplicate under the same key is answered.
 */
export interface SubmissionResolutionExecuted {
  "horizon": string;
  "outcome": "executed";
  "reply": Reply;
}

/**
 * It never ran: no receipt, and no receipt recorded since the request's
 * horizon was ever dropped. Sending it again under the same key is safe.
 */
export interface SubmissionResolutionAbsentSafe {
  "horizon": string;
  "outcome": "absent_safe";
  "retained_since": string;
}

/**
 * No receipt, and nothing proves there never was one. Only a person can
 * decide whether to send it again.
 */
export interface SubmissionResolutionUnknown {
  "horizon": string;
  "outcome": "unknown";
  "reason": UnprovenAbsence;
  "retained_since": string;
}

export type SubmissionResolution =
  | SubmissionResolutionExecuted
  | SubmissionResolutionAbsentSafe
  | SubmissionResolutionUnknown;

/**
 * A durable task record.
 */
export interface Task {
  "artifacts"?: Array<ArtifactId>;
  "created_at": string;
  "creator": Actor;
  "dependencies"?: Array<TaskId>;
  "finished_at"?: string | null;
  "id": TaskId;
  "objective": string;
  "owner"?: AgentId | null;
  "parent_task"?: TaskId | null;
  "priority"?: Priority;
  "run": RunId;
  "started_at"?: string | null;
  "status": TaskStatus;
  "updated_at": string;
}

/**
 * A tsk_ prefixed ULID.
 * Pattern: ^tsk_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type TaskId = string;

export interface TaskLine {
  "created_at": string;
  "dependencies"?: Array<TaskId>;
  "ended_at"?: string | null;
  "id": TaskId;
  "objective": string;
  "outcome": Ended;
  "owner"?: AgentId | null;
  "parent_task"?: TaskId | null;
  "priority": Priority;
}

export interface TaskQuery {
  "owner"?: AgentId | null;
  "run"?: RunId | null;
  "statuses"?: Array<TaskStatusKind>;
}

/**
 * The durable outcome of a task. Cheap to carry in context forever.
 */
export interface TaskResult {
  "artifacts"?: Array<ArtifactId>;
  "structured"?: unknown;
  "summary": string;
}

/**
 * Tagged by `state`.
 */
export interface TaskStatusQueued {
  "state": "queued";
}

export interface TaskStatusAssigned {
  "state": "assigned";
}

export interface TaskStatusActive {
  "state": "active";
}

export interface TaskStatusBlocked {
  "dependency"?: TaskId | null;
  "reason": string;
  "state": "blocked";
}

export interface TaskStatusCompleted {
  "result": TaskResult;
  "state": "completed";
}

export interface TaskStatusFailed {
  "error": string;
  "state": "failed";
  "structured"?: unknown;
}

export interface TaskStatusCancelled {
  "reason"?: string | null;
  "state": "cancelled";
}

export type TaskStatus =
  | TaskStatusQueued
  | TaskStatusAssigned
  | TaskStatusActive
  | TaskStatusBlocked
  | TaskStatusCompleted
  | TaskStatusFailed
  | TaskStatusCancelled;

export type TaskStatusKind = "queued" | "assigned" | "active" | "blocked" | "completed" | "failed" | "cancelled";

/**
 * How the model should think. Adapters honor what the capability profile
 * supports and report the rest as unsupported.
 * Tagged by `mode`.
 */
export interface ThinkingModeOff {
  "mode": "off";
}

export interface ThinkingModeAdaptive {
  "mode": "adaptive";
}

export interface ThinkingModeBudget {
  "mode": "budget";
  "tokens": number;
}

export type ThinkingMode = ThinkingModeOff | ThinkingModeAdaptive | ThinkingModeBudget;

/**
 * Which thinking controls a model accepts.
 */
export type ThinkingSupport = "none" | "budget" | "adaptive";

/**
 * One tool invocation requested by a model.
 */
export interface ToolCall {
  "agent_id": AgentId;
  "id": ToolCallId;
  "idempotency_key"?: IdempotencyKey | null;
  "input": unknown;
  "invocation_id"?: InvocationId | null;
  "name": string;
  "provider_call_id": string;
  "requested_at": string;
  "resources"?: Array<ResourceKey>;
  "task_id"?: TaskId | null;
}

/**
 * A call_ prefixed ULID.
 * Pattern: ^call_[0-9A-HJKMNP-TV-Z]{26}$
 */
export type ToolCallId = string;

/**
 * Which registered tools an agent may see.
 * Tagged by `mode`.
 */
export interface ToolExposureAll {
  "mode": "all";
}

export interface ToolExposureOnly {
  "mode": "only";
  "names": Array<string>;
}

export interface ToolExposureExcept {
  "mode": "except";
  "names": Array<string>;
}

export type ToolExposure = ToolExposureAll | ToolExposureOnly | ToolExposureExcept;

/**
 * One tool's tally across the run.
 */
export interface ToolLine {
  "cancelled": number;
  "denied": number;
  "duration_ms": number;
  "failed": number;
  "replayed": number;
  "requested": number;
  "retries": number;
  "succeeded": number;
  "tool": string;
}

/**
 * What a tool produced.
 */
export interface ToolOutcome {
  "artifacts"?: Array<ArtifactId>;
  "content": string;
  "duration_ms"?: number;
  "is_error"?: boolean;
  "side_effect_occurred"?: boolean | null;
  "structured"?: unknown;
  "truncated"?: boolean;
}

/**
 * How a tool call in this turn ended.
 * Tagged by `result`.
 */
/**
 * Requested, and the log carries no end for it.
 */
export interface ToolResultPending {
  "result": "pending";
}

export interface ToolResultSucceeded {
  "artifacts"?: Array<ArtifactId>;
  "content": string;
  "duration_ms": number;
  "is_error"?: boolean;
  "result": "succeeded";
  "truncated"?: boolean;
}

export interface ToolResultFailed {
  "duration_ms"?: number | null;
  "error": string;
  "result": "failed";
  "retryable": boolean;
}

export interface ToolResultDenied {
  "reason": string;
  "result": "denied";
}

export interface ToolResultCancelled {
  "result": "cancelled";
}

/**
 * Answered from a committed outcome; the call did not run again.
 */
export interface ToolResultReplayed {
  "original": ToolCallId;
  "result": "replayed";
}

export type ToolResult =
  | ToolResultPending
  | ToolResultSucceeded
  | ToolResultFailed
  | ToolResultDenied
  | ToolResultCancelled
  | ToolResultReplayed;

/**
 * Content allowed inside a tool result.
 * Tagged by `type`.
 */
export interface ToolResultBlockText {
  "text": string;
  "type": "text";
}

export interface ToolResultBlockImage {
  "data": string;
  "media_type": string;
  "type": "image";
}

export type ToolResultBlock = ToolResultBlockText | ToolResultBlockImage;

/**
 * Tool definition as exposed to the model.
 */
export interface ToolSpec {
  "description": string;
  "input_schema": unknown;
  "name": string;
  "strict"?: boolean;
}

export interface TraceCandidate {
  "memory": MemoryId;
  "outcome": TraceOutcome;
  "pinned": boolean;
  "rank"?: number | null;
  "relevance": number;
  "score"?: number | null;
  "stale": boolean;
}

/**
 * Candidates past the trace's caps, by the outcome they had.
 */
export interface TraceOmitted {
  "cut": number;
  "over_limit": number;
}

export type TraceOutcome = "injected" | "held_out" | "trimmed" | "over_limit" | "cut";

export interface TracePolicy {
  "holdout_rate": number;
  "max_items": number;
  "min_score"?: number | null;
}

export interface TranscriptEntry {
  "at": string;
  "invocation"?: InvocationId | null;
  "message": Message;
  "seq": number;
}

/**
 * One node of the live graph.
 */
export interface TreeNode {
  "agent": Agent;
  "children": Array<AgentId>;
  "hidden_usage"?: Usage | null;
  "lease"?: Lease | null;
  "subtree_usage": Usage;
  "task"?: Task | null;
  "waiting_on"?: Array<TaskId>;
}

/**
 * How the turn ended.
 * Tagged by `outcome`.
 */
/**
 * The log ends inside this turn: it is the turn in flight.
 */
export interface TurnOutcomeRunning {
  "outcome": "running";
}

export interface TurnOutcomeCompleted {
  "outcome": "completed";
  "stop_reason"?: StopReason | null;
}

export interface TurnOutcomeFailed {
  "error": string;
  "outcome": "failed";
  "retryable": boolean;
}

export interface TurnOutcomeInterrupted {
  "outcome": "interrupted";
}

/**
 * No turn has started: a turn-0 frame holding the notes written so far.
 */
export interface TurnOutcomeNotStarted {
  "outcome": "not_started";
}

export type TurnOutcome =
  | TurnOutcomeRunning
  | TurnOutcomeCompleted
  | TurnOutcomeFailed
  | TurnOutcomeInterrupted
  | TurnOutcomeNotStarted;

/**
 * Why an absent receipt does not prove its key was never executed.
 */
export type UnprovenAbsence = "no_horizon" | "foreign_horizon" | "horizon_ahead" | "forgotten";

/**
 * Token accounting for one invocation, or a running total.
 *
 * `cost_usd` sums invocations with a configured price or provider-reported
 * cost; `cost_basis` distinguishes those sources and `cost_complete` says
 * whether any invocations were left out. List prices are estimates, even
 * when every invocation was priced. A budget enforces on this accounting
 * bound - an unpriced invocation
 * cannot make what was already spent smaller - so nothing silently stops
 * firing when a model or engine reports no price.
 */
export interface Usage {
  "cache_read_tokens": number;
  "cache_write_1h_tokens"?: number;
  "cache_write_tokens": number;
  "cost_basis"?: CostBasis;
  "cost_complete": boolean;
  "cost_gap"?: CostGap | null;
  "cost_usd": number;
  "estimated_cost_usd"?: number | null;
  "input_tokens": number;
  "output_tokens": number;
  "reasoning_tokens"?: number | null;
}

/**
 * Reply to `usage_breakdown`: rows ordered by `key`, and the total over
 * every invocation in scope, those of rows cut by `limit` included.
 */
export interface UsageBreakdown {
  "rows": Array<UsageRow>;
  "total": Usage;
  "truncated": boolean;
}

/**
 * What each `usage_breakdown` row is keyed by.
 */
export type UsageGroup = "provider" | "run" | "model" | "role" | "day";

/**
 * One group's spend.
 */
export interface UsageRow {
  "invocations": number;
  "key": string;
  "usage": Usage;
}

/**
 * Which invocations `usage_breakdown` sums. Every field given narrows the
 * set; none sums the whole fleet.
 */
export interface UsageScope {
  "agent"?: AgentId | null;
  "role_prefix"?: string | null;
  "run"?: RunId | null;
  "subtree"?: AgentId | null;
}

/**
 * One named account limit, including limits outside the preferred scheduling window.
 */
export interface UsageWindow {
  "name": string;
  "quota": QuotaWindow;
}

/**
 * Ordered weakest first, so the stronger of two is their maximum.
 */
export type Verification = "inferred" | "observed" | "verified";

export interface ViewMode {
  "builtin": boolean;
  "density": ViewModeDensity;
  "description"?: string | null;
  "label"?: string | null;
  "name": string;
  "scope"?: ViewModeScope | null;
  "show": Array<ViewModeCategory>;
}

export type ViewModeCategory = "user_text" | "assistant_text" | "delegation" | "agent_message" | "completion" | "status" | "question" | "approval" | "error" | "tool" | "reasoning" | "memory" | "compaction" | "usage";

export type ViewModeDensity = "comfortable" | "compact";

export interface ViewModeDiagnostic {
  "reason": string;
  "source": string;
}

export interface ViewModeScope {
  "name"?: string | null;
  "role"?: string | null;
}

export interface ViewModes {
  "diagnostics": Array<ViewModeDiagnostic>;
  "modes": Array<ViewMode>;
}

/**
 * What a waiting agent is waiting for. A typed expression the scheduler
 * evaluates; the model never polls. Leaves are single facts, and `Any`,
 * `All` and `AtLeast` combine them, so "wait for two of these five workers
 * or a deadline" is a value, not a prompt.
 * Tagged by `on`.
 */
/**
 * The task reached a terminal state.
 */
export interface WaitConditionTask {
  "id": TaskId;
  "on": "task";
}

export interface WaitConditionApproval {
  "id": ApprovalId;
  "on": "approval";
}

export interface WaitConditionMessageFrom {
  "agent": AgentId;
  "on": "message_from";
}

/**
 * Any inbox item at all.
 */
export interface WaitConditionInbox {
  "on": "inbox";
}

export interface WaitConditionDeadline {
  "at": string;
  "on": "deadline";
}

export interface WaitConditionAny {
  "of": Array<WaitCondition>;
  "on": "any";
}

export interface WaitConditionAll {
  "of": Array<WaitCondition>;
  "on": "all";
}

export interface WaitConditionAtLeast {
  "n": number;
  "of": Array<WaitCondition>;
  "on": "at_least";
}

export type WaitCondition =
  | WaitConditionTask
  | WaitConditionApproval
  | WaitConditionMessageFrom
  | WaitConditionInbox
  | WaitConditionDeadline
  | WaitConditionAny
  | WaitConditionAll
  | WaitConditionAtLeast;

export interface Welcome {
  "daemon_protocol_version"?: number | null;
  "daemon_version": string;
  "device"?: DeviceIdentity | null;
  "features"?: Array<string>;
  "idle_timeout_secs"?: number | null;
  "instance_id": string;
  "min_protocol_version"?: number | null;
  "pid": number;
  "protocol_version": number;
  "schema_version": number;
  "submission_horizon"?: string | null;
}

export interface WindowHeadroom {
  "name": string;
  "remaining_percent": number;
  "resets_at"?: string | null;
  "resets_in_seconds"?: number | null;
  "window_minutes"?: number | null;
}

/**
 * One workspace the daemon knows: it holds or held a root agent.
 */
export interface Workspace {
  "active_at": string;
  "agents": number;
  "exists": boolean;
  "git"?: GitSummary | null;
  "path": string;
  "running": number;
}

export interface WorkspaceCommit {
  "at": string;
  "author": string;
  "id": string;
  "summary": string;
  "turn"?: number | null;
}

/**
 * An agent's work in its workspace, bounded for the wire.
 */
export interface WorkspaceDiff {
  "agent": AgentId;
  "ambiguous": boolean;
  "commits": Array<WorkspaceCommit>;
  "commits_truncated": boolean;
  "deletions": number;
  "files": Array<ChangedFile>;
  "files_truncated": boolean;
  "insertions": number;
  "integrated": boolean;
  "isolated": boolean;
  "patch": string;
  "patch_bytes": number;
  "patch_truncated": boolean;
  "source"?: string | null;
  "workspace": string;
}

export interface WorkspaceDiscard {
  "agent": AgentId;
  "forced": boolean;
  "workspace": string;
}

/**
 * The result of an operator's integration. Refused with `conflicts` when
 * the source changed a path the child also changed; nothing was written.
 */
export interface WorkspaceIntegration {
  "agent": AgentId;
  "already_integrated": boolean;
  "changed_paths": Array<string>;
  "conflicts": Array<string>;
  "integrated": boolean;
  "source": string;
}

export interface WorkspaceValidation {
  "git"?: GitSummary | null;
  "path"?: string | null;
  "reason"?: string | null;
  "valid": boolean;
}

/**
 * Payload bytes the store committed since `since`, heaviest kind first.
 * Counts JSON bodies, not SQLite pages: indexes and the WAL add on top, so
 * the file sizes are reported next to it.
 */
export interface WriteStats {
  "db_bytes"?: number | null;
  "kinds": Array<KindWrites>;
  "since"?: string | null;
  "total_bytes": number;
  "total_rows": number;
  "wal_bytes"?: number | null;
}

// Commands. Each builds one frame; the client adds the request id.

/**
 * Accept a pending proposal and apply its deferred replacement. Needs `memory_proposals`.
 */
export interface AcceptMemoryCommand {
  "cmd": "accept_memory";
  "id": MemoryId;
  "reason"?: string | null;
}

export function acceptMemory(args: Omit<AcceptMemoryCommand, "cmd">): AcceptMemoryCommand {
  return { cmd: "accept_memory", ...args };
}

export interface AddApprovalRuleCommand {
  "cmd": "add_approval_rule";
  "decision": ApprovalDecision;
  "idempotency_key"?: string | null;
  "match": RuleMatch;
  "note"?: string | null;
  "scope": RuleScope;
}

export function addApprovalRule(args: Omit<AddApprovalRuleCommand, "cmd">): AddApprovalRuleCommand {
  return { cmd: "add_approval_rule", ...args };
}

/**
 * Make `alias`, an old repository id, resolve to the repository at
 * `root` or to the id `repository`, and move every memory filed under
 * the alias there. The move is one-way. Needs `repository_aliases`.
 */
export interface AliasRepositoryCommand {
  "cmd": "alias_repository";
  "alias": string;
  "dry_run"?: boolean;
  "repository"?: string | null;
  "root"?: string | null;
}

export function aliasRepository(args: Omit<AliasRepositoryCommand, "cmd">): AliasRepositoryCommand {
  return { cmd: "alias_repository", ...args };
}

/**
 * Answer a notification with one of its actions, free text where it
 * takes it, or both; this resolves it and reaches a posting agent's
 * inbox. Needs `features::NOTIFICATIONS`.
 */
export interface AnswerNotificationCommand {
  "cmd": "answer_notification";
  "action"?: string | null;
  "id": NotificationId;
  "text"?: string | null;
}

export function answerNotification(args: Omit<AnswerNotificationCommand, "cmd">): AnswerNotificationCommand {
  return { cmd: "answer_notification", ...args };
}

/**
 * Append a note to an agent's transcript without starting a turn. People
 * see it at once; the model sees it only with `context: digest`, in the
 * next turn's runtime state. Needs `features::NOTES`.
 */
export interface AppendNoteCommand {
  "cmd": "append_note";
  "agent": AgentId;
  "context"?: NoteContext;
  "level"?: NoteLevel;
  "links"?: Array<string>;
  "text": string;
}

export function appendNote(args: Omit<AppendNoteCommand, "cmd">): AppendNoteCommand {
  return { cmd: "append_note", ...args };
}

/**
 * Clear finished work out of the live view, or bring it back. The mark
 * is durable and hides nothing from the store, reports or replay.
 * Only a finished agent can be archived.
 */
export interface ArchiveCommand {
  "cmd": "archive";
  "archived"?: boolean;
  "idempotency_key"?: string | null;
  "target": ArchiveTarget;
}

export function archive(args: Omit<ArchiveCommand, "cmd">): ArchiveCommand {
  return { cmd: "archive", ...args };
}

export interface AuthStatusCommand {
  "cmd": "auth_status";
}

export function authStatus(): AuthStatusCommand {
  return { cmd: "auth_status" };
}

export interface CancelCommand {
  "cmd": "cancel";
  "agent": AgentId;
  "idempotency_key"?: string | null;
  "reason"?: string | null;
}

export function cancel(args: Omit<CancelCommand, "cmd">): CancelCommand {
  return { cmd: "cancel", ...args };
}

/**
 * Close a pairing nobody confirmed. Host only.
 */
export interface CancelPairingCommand {
  "cmd": "cancel_pairing";
  "id": string;
}

export function cancelPairing(args: Omit<CancelPairingCommand, "cmd">): CancelPairingCommand {
  return { cmd: "cancel_pairing", ...args };
}

/**
 * Ask the runtime to compact the agent's context before its next turn.
 */
export interface CompactCommand {
  "cmd": "compact";
  "agent": AgentId;
}

export function compact(args: Omit<CompactCommand, "cmd">): CompactCommand {
  return { cmd: "compact", ...args };
}

/**
 * Complete a partial path on the daemon's machine. Bounded: at most
 * `limit` entries (default 50, never more than 200).
 */
export interface CompletePathCommand {
  "cmd": "complete_path";
  "kind": PathCompletionKind;
  "limit"?: number | null;
  "prefix": string;
  "workspace"?: string | null;
}

export function completePath(args: Omit<CompletePathCommand, "cmd">): CompletePathCommand {
  return { cmd: "complete_path", ...args };
}

/**
 * Replace watcher settings only if no other operator changed them.
 */
export interface ConfigureGithubWatcherCommand {
  "cmd": "configure_github_watcher";
  "config": GithubWatcherConfig;
  "expected_revision": number;
}

export function configureGithubWatcher(args: Omit<ConfigureGithubWatcherCommand, "cmd">): ConfigureGithubWatcherCommand {
  return { cmd: "configure_github_watcher", ...args };
}

/**
 * Replace the remote settings only if nobody else changed them. Opens
 * or closes the listener at once. Host only.
 */
export interface ConfigureRemoteCommand {
  "cmd": "configure_remote";
  "expected_revision": number;
  "settings": RemoteSettings;
}

export function configureRemote(args: Omit<ConfigureRemoteCommand, "cmd">): ConfigureRemoteCommand {
  return { cmd: "configure_remote", ...args };
}

/**
 * Enrol or reject the device that claimed a pairing. Host only.
 */
export interface ConfirmPairingCommand {
  "cmd": "confirm_pairing";
  "accept": boolean;
  "id": string;
}

export function confirmPairing(args: Omit<ConfirmPairingCommand, "cmd">): ConfirmPairingCommand {
  return { cmd: "confirm_pairing", ...args };
}

/**
 * Merge near-duplicate memories, or with `dry_run` only plan it.
 * An absent scope means every scope. Needs `memory_v2`.
 */
export interface ConsolidateMemoriesCommand {
  "cmd": "consolidate_memories";
  "dry_run": boolean;
  "scope"?: MemoryScope | null;
}

export function consolidateMemories(args: Omit<ConsolidateMemoriesCommand, "cmd">): ConsolidateMemoriesCommand {
  return { cmd: "consolidate_memories", ...args };
}

export interface CreateAgentCommand {
  "cmd": "create_agent";
  "approval"?: ApprovalPolicy | null;
  "config"?: AgentConfig | null;
  "fallbacks"?: Array<ModelRef>;
  "idempotency_key"?: string | null;
  "model"?: ModelRef | null;
  "name"?: string | null;
  "objective"?: string | null;
  "parent"?: AgentId | null;
  "repository_root"?: string | null;
  "role": string;
  "run"?: RunId | null;
  "scope"?: Scope | null;
  "start"?: boolean;
}

export function createAgent(args: Omit<CreateAgentCommand, "cmd">): CreateAgentCommand {
  return { cmd: "create_agent", ...args };
}

/**
 * Admit an operator-authored fact. Rejections return an error without mutation.
 */
export interface CreateMemoryCommand {
  "cmd": "create_memory";
  "idempotency_key"?: string | null;
  "spec": OperatorMemorySpec;
}

export function createMemory(args: Omit<CreateMemoryCommand, "cmd">): CreateMemoryCommand {
  return { cmd: "create_memory", ...args };
}

export interface CreateScheduleCommand {
  "cmd": "create_schedule";
  "idempotency_key"?: string | null;
  "spec": CreateScheduleSpec;
}

export function createSchedule(args: Omit<CreateScheduleCommand, "cmd">): CreateScheduleCommand {
  return { cmd: "create_schedule", ...args };
}

export interface CreateTaskCommand {
  "cmd": "create_task";
  "dependencies"?: Array<TaskId>;
  "idempotency_key"?: string | null;
  "objective": string;
  "owner"?: AgentId | null;
  "parent_task"?: TaskId | null;
  "priority"?: Priority;
}

export function createTask(args: Omit<CreateTaskCommand, "cmd">): CreateTaskCommand {
  return { cmd: "create_task", ...args };
}

/**
 * A harness tool call or permission prompt from an external engine's
 * MCP bridge, authenticated by the per-turn token.
 */
export interface EngineToolCommand {
  "cmd": "engine_tool";
  "agent": AgentId;
  "input": unknown;
  "name": string;
  "token": string;
}

export function engineTool(args: Omit<EngineToolCommand, "cmd">): EngineToolCommand {
  return { cmd: "engine_tool", ...args };
}

/**
 * The tools an engine's MCP bridge should advertise for this agent:
 * Aether's harness tools plus the MCP servers' tools it may use.
 */
export interface EngineToolsCommand {
  "cmd": "engine_tools";
  "agent": AgentId;
  "token": string;
}

export function engineTools(args: Omit<EngineToolsCommand, "cmd">): EngineToolsCommand {
  return { cmd: "engine_tools", ...args };
}

/**
 * Write a finished run's events, snapshots, checkpoints and artifacts
 * to one file on the daemon's machine.
 */
export interface ExportRunCommand {
  "cmd": "export_run";
  "path": string;
  "run": RunId;
}

export function exportRun(args: Omit<ExportRunCommand, "cmd">): ExportRunCommand {
  return { cmd: "export_run", ...args };
}

/**
 * Curate a memory out. The record stays; it is never retrieved again.
 */
export interface ForgetMemoryCommand {
  "cmd": "forget_memory";
  "id": MemoryId;
  "reason"?: string | null;
}

export function forgetMemory(args: Omit<ForgetMemoryCommand, "cmd">): ForgetMemoryCommand {
  return { cmd: "forget_memory", ...args };
}

export interface GetAgentCommand {
  "cmd": "get_agent";
  "id": AgentId;
}

export function getAgent(args: Omit<GetAgentCommand, "cmd">): GetAgentCommand {
  return { cmd: "get_agent", ...args };
}

export interface GetArtifactCommand {
  "cmd": "get_artifact";
  "id": ArtifactId;
}

export function getArtifact(args: Omit<GetArtifactCommand, "cmd">): GetArtifactCommand {
  return { cmd: "get_artifact", ...args };
}

/**
 * Everything waiting on the operator: pending approvals, unanswered
 * questions, and failed or blocked agents, read at one sequence number.
 */
export interface GetAttentionCommand {
  "cmd": "get_attention";
}

export function getAttention(): GetAttentionCommand {
  return { cmd: "get_attention" };
}

export interface GetCheckpointCommand {
  "cmd": "get_checkpoint";
  "id": CheckpointId;
}

export function getCheckpoint(args: Omit<GetCheckpointCommand, "cmd">): GetCheckpointCommand {
  return { cmd: "get_checkpoint", ...args };
}

export interface GetEventsCommand {
  "cmd": "get_events";
  "after": number;
  "limit": number;
  "query": EventQuery;
}

export function getEvents(args: Omit<GetEventsCommand, "cmd">): GetEventsCommand {
  return { cmd: "get_events", ...args };
}

/**
 * Branch and dirtiness of a workspace on the daemon's machine.
 */
export interface GetGitSummaryCommand {
  "cmd": "get_git_summary";
  "workspace": string;
}

export function getGitSummary(args: Omit<GetGitSummaryCommand, "cmd">): GetGitSummaryCommand {
  return { cmd: "get_git_summary", ...args };
}

/**
 * Inspect the opt-in GitHub watcher and its per-repository health.
 */
export interface GetGithubWatcherCommand {
  "cmd": "get_github_watcher";
}

export function getGithubWatcher(): GetGithubWatcherCommand {
  return { cmd: "get_github_watcher" };
}

export interface GetInboxCommand {
  "cmd": "get_inbox";
  "after": number;
  "agent": AgentId;
}

export function getInbox(args: Omit<GetInboxCommand, "cmd">): GetInboxCommand {
  return { cmd: "get_inbox", ...args };
}

export interface GetInvocationCommand {
  "cmd": "get_invocation";
  "id": InvocationId;
}

export function getInvocation(args: Omit<GetInvocationCommand, "cmd">): GetInvocationCommand {
  return { cmd: "get_invocation", ...args };
}

/**
 * One notification. Needs `features::NOTIFICATIONS`.
 */
export interface GetNotificationCommand {
  "cmd": "get_notification";
  "id": NotificationId;
}

export function getNotification(args: Omit<GetNotificationCommand, "cmd">): GetNotificationCommand {
  return { cmd: "get_notification", ...args };
}

/**
 * Read a pairing's state; the code is never repeated. Host only.
 */
export interface GetPairingCommand {
  "cmd": "get_pairing";
  "id": string;
}

export function getPairing(args: Omit<GetPairingCommand, "cmd">): GetPairingCommand {
  return { cmd: "get_pairing", ...args };
}

/**
 * Whether the daemon sends Web Push, and the key to subscribe with.
 */
export interface GetPushConfigCommand {
  "cmd": "get_push_config";
}

export function getPushConfig(): GetPushConfigCommand {
  return { cmd: "get_push_config" };
}

/**
 * The remote listener's settings and state. Host only.
 */
export interface GetRemoteCommand {
  "cmd": "get_remote";
}

export function getRemote(): GetRemoteCommand {
  return { cmd: "get_remote" };
}

/**
 * Compose a run's report from its events.
 */
export interface GetRunReportCommand {
  "cmd": "get_run_report";
  "run": RunId;
}

export function getRunReport(args: Omit<GetRunReportCommand, "cmd">): GetRunReportCommand {
  return { cmd: "get_run_report", ...args };
}

export interface GetScheduleCommand {
  "cmd": "get_schedule";
  "id": ScheduleId;
}

export function getSchedule(args: Omit<GetScheduleCommand, "cmd">): GetScheduleCommand {
  return { cmd: "get_schedule", ...args };
}

export interface GetSnapshotCommand {
  "cmd": "get_snapshot";
  "id": SnapshotId;
}

export function getSnapshot(args: Omit<GetSnapshotCommand, "cmd">): GetSnapshotCommand {
  return { cmd: "get_snapshot", ...args };
}

export interface GetTaskCommand {
  "cmd": "get_task";
  "id": TaskId;
}

export function getTask(args: Omit<GetTaskCommand, "cmd">): GetTaskCommand {
  return { cmd: "get_task", ...args };
}

export interface GetTranscriptCommand {
  "cmd": "get_transcript";
  "after": number;
  "agent": AgentId;
}

export function getTranscript(args: Omit<GetTranscriptCommand, "cmd">): GetTranscriptCommand {
  return { cmd: "get_transcript", ...args };
}

/**
 * A bounded page of an agent's transcript: entries after `after`, the
 * newest before `before`, or the newest of all when neither is given.
 * `limit` is 1 to 1000, 200 when absent. Needs `features::TRANSCRIPT_PAGING`.
 */
export interface GetTranscriptPageCommand {
  "cmd": "get_transcript_page";
  "after"?: number | null;
  "agent": AgentId;
  "before"?: number | null;
  "limit"?: number | null;
}

export function getTranscriptPage(args: Omit<GetTranscriptPageCommand, "cmd">): GetTranscriptPageCommand {
  return { cmd: "get_transcript_page", ...args };
}

/**
 * The live graph: every agent in the run (or all runs) with relations.
 */
export interface GetTreeCommand {
  "cmd": "get_tree";
  "include_archived"?: boolean;
  "role_prefix"?: string | null;
  "root"?: AgentId | null;
  "run"?: RunId | null;
}

export function getTree(args: Omit<GetTreeCommand, "cmd">): GetTreeCommand {
  return { cmd: "get_tree", ...args };
}

export interface HelloCommand {
  "cmd": "hello";
  "client_name": string;
  "client_version": string;
  "device"?: DeviceProof | null;
  "features"?: Array<string>;
  "protocol_version": number;
  "token"?: string | null;
}

export function hello(args: Omit<HelloCommand, "cmd">): HelloCommand {
  return { cmd: "hello", ...args };
}

/**
 * Recreate a run from such a file, under fresh ids.
 */
export interface ImportRunCommand {
  "cmd": "import_run";
  "path": string;
}

export function importRun(args: Omit<ImportRunCommand, "cmd">): ImportRunCommand {
  return { cmd: "import_run", ...args };
}

/**
 * Stop the running turn only; the agent keeps its transcript and waits
 * for the next message.
 */
export interface InterruptCommand {
  "cmd": "interrupt";
  "agent": AgentId;
  "idempotency_key"?: string | null;
}

export function interrupt(args: Omit<InterruptCommand, "cmd">): InterruptCommand {
  return { cmd: "interrupt", ...args };
}

export interface LatestCheckpointCommand {
  "cmd": "latest_checkpoint";
  "agent": AgentId;
}

export function latestCheckpoint(args: Omit<LatestCheckpointCommand, "cmd">): LatestCheckpointCommand {
  return { cmd: "latest_checkpoint", ...args };
}

export interface ListAgentsCommand {
  "cmd": "list_agents";
  "query": AgentQuery;
}

export function listAgents(args: Omit<ListAgentsCommand, "cmd">): ListAgentsCommand {
  return { cmd: "list_agents", ...args };
}

/**
 * One page of agents in `AgentId` order, from just after `after`.
 * `limit` is 1 to 500, 200 when absent. Needs `features::AGENTS_PAGING`.
 */
export interface ListAgentsPageCommand {
  "cmd": "list_agents_page";
  "after"?: AgentId | null;
  "limit"?: number | null;
  "query"?: AgentQuery;
}

export function listAgentsPage(args: Omit<ListAgentsPageCommand, "cmd">): ListAgentsPageCommand {
  return { cmd: "list_agents_page", ...args };
}

/**
 * Remembered decisions, oldest first.
 */
export interface ListApprovalRulesCommand {
  "cmd": "list_approval_rules";
  "include_revoked"?: boolean;
}

export function listApprovalRules(args: Omit<ListApprovalRulesCommand, "cmd">): ListApprovalRulesCommand {
  return { cmd: "list_approval_rules", ...args };
}

export interface ListApprovalsCommand {
  "cmd": "list_approvals";
}

export function listApprovals(): ListApprovalsCommand {
  return { cmd: "list_approvals" };
}

export interface ListArtifactsCommand {
  "cmd": "list_artifacts";
  "query": ArtifactQuery;
}

export function listArtifacts(args: Omit<ListArtifactsCommand, "cmd">): ListArtifactsCommand {
  return { cmd: "list_artifacts", ...args };
}

/**
 * Enrolled devices, oldest first. Host only.
 */
export interface ListDevicesCommand {
  "cmd": "list_devices";
  "include_revoked"?: boolean;
}

export function listDevices(args: Omit<ListDevicesCommand, "cmd">): ListDevicesCommand {
  return { cmd: "list_devices", ...args };
}

export interface ListInvocationsCommand {
  "cmd": "list_invocations";
  "agent": AgentId;
}

export function listInvocations(args: Omit<ListInvocationsCommand, "cmd">): ListInvocationsCommand {
  return { cmd: "list_invocations", ...args };
}

export interface ListModelsCommand {
  "cmd": "list_models";
}

export function listModels(): ListModelsCommand {
  return { cmd: "list_models" };
}

/**
 * Notifications newest first. Needs `features::NOTIFICATIONS`.
 */
export interface ListNotificationsCommand {
  "cmd": "list_notifications";
  "limit"?: number | null;
  "source"?: string | null;
  "states"?: Array<NotificationState>;
}

export function listNotifications(args: Omit<ListNotificationsCommand, "cmd">): ListNotificationsCommand {
  return { cmd: "list_notifications", ...args };
}

/**
 * Registered repository aliases. Needs `repository_aliases`.
 */
export interface ListRepositoryAliasesCommand {
  "cmd": "list_repository_aliases";
}

export function listRepositoryAliases(): ListRepositoryAliasesCommand {
  return { cmd: "list_repository_aliases" };
}

/**
 * Runs this store knows about, newest first.
 */
export interface ListRunsCommand {
  "cmd": "list_runs";
  "limit": number;
}

export function listRuns(args: Omit<ListRunsCommand, "cmd">): ListRunsCommand {
  return { cmd: "list_runs", ...args };
}

export interface ListSchedulesCommand {
  "cmd": "list_schedules";
  "target"?: AgentId | null;
}

export function listSchedules(args: Omit<ListSchedulesCommand, "cmd">): ListSchedulesCommand {
  return { cmd: "list_schedules", ...args };
}

export interface ListTasksCommand {
  "cmd": "list_tasks";
  "query": TaskQuery;
}

export function listTasks(args: Omit<ListTasksCommand, "cmd">): ListTasksCommand {
  return { cmd: "list_tasks", ...args };
}

/**
 * Built-in and user transcript modes from the daemon's configuration.
 */
export interface ListViewModesCommand {
  "cmd": "list_view_modes";
}

export function listViewModes(): ListViewModesCommand {
  return { cmd: "list_view_modes" };
}

/**
 * Workspaces holding root agents, most recently active first.
 */
export interface ListWorkspacesCommand {
  "cmd": "list_workspaces";
  "limit"?: number | null;
}

export function listWorkspaces(args: Omit<ListWorkspacesCommand, "cmd">): ListWorkspacesCommand {
  return { cmd: "list_workspaces", ...args };
}

/**
 * Mark a notification seen. Needs `features::NOTIFICATIONS`.
 */
export interface MarkNotificationReadCommand {
  "cmd": "mark_notification_read";
  "id": NotificationId;
}

export function markNotificationRead(args: Omit<MarkNotificationReadCommand, "cmd">): MarkNotificationReadCommand {
  return { cmd: "mark_notification_read", ...args };
}

/**
 * Configured MCP servers and their tools. `refresh` reconnects first,
 * so tools added since daemon start are discovered on demand.
 */
export interface McpServersCommand {
  "cmd": "mcp_servers";
  "refresh"?: boolean;
  "server"?: string | null;
}

export function mcpServers(args: Omit<McpServersCommand, "cmd">): McpServersCommand {
  return { cmd: "mcp_servers", ...args };
}

export interface MemoriesCommand {
  "cmd": "memories";
  "query": MemoryQuerySpec;
}

export function memories(args: Omit<MemoriesCommand, "cmd">): MemoriesCommand {
  return { cmd: "memories", ...args };
}

/**
 * Which stored memories every model call of a task, or one invocation,
 * was given. Name exactly one. Needs `memory_context`.
 */
export interface MemoriesInContextCommand {
  "cmd": "memories_in_context";
  "invocation"?: InvocationId | null;
  "task"?: TaskId | null;
}

export function memoriesInContext(args: Omit<MemoriesInContextCommand, "cmd">): MemoriesInContextCommand {
  return { cmd: "memories_in_context", ...args };
}

/**
 * Every memory of a scope, or of the store, as a JSON Lines artifact.
 * Needs `memory_v2`.
 */
export interface MemoryExportCommand {
  "cmd": "memory_export";
  "scope"?: MemoryScope | null;
}

export function memoryExport(args: Omit<MemoryExportCommand, "cmd">): MemoryExportCommand {
  return { cmd: "memory_export", ...args };
}

/**
 * Newest evidence first, including retractions. Needs `memory_feedback`.
 */
export interface MemoryFeedbackCommand {
  "cmd": "memory_feedback";
  "limit"?: number | null;
  "memory": MemoryId;
}

export function memoryFeedback(args: Omit<MemoryFeedbackCommand, "cmd">): MemoryFeedbackCommand {
  return { cmd: "memory_feedback", ...args };
}

/**
 * Insert what an export holds and the store lacks; never overwrites.
 * Needs `memory_v2`.
 */
export interface MemoryImportCommand {
  "cmd": "memory_import";
  "artifact": string;
  "dry_run": boolean;
}

export function memoryImport(args: Omit<MemoryImportCommand, "cmd">): MemoryImportCommand {
  return { cmd: "memory_import", ...args };
}

/**
 * The store's Memory 2.0 migrations, counts and backups. Needs `memory_v2`.
 */
export interface MemoryMigrationStatusCommand {
  "cmd": "memory_migration_status";
}

export function memoryMigrationStatus(): MemoryMigrationStatusCommand {
  return { cmd: "memory_migration_status" };
}

/**
 * What retrieval did for each model call of a task, or for one
 * invocation; name exactly one. Needs `memory_v2`.
 */
export interface MemoryRetrievalExplainCommand {
  "cmd": "memory_retrieval_explain";
  "invocation"?: InvocationId | null;
  "task"?: TaskId | null;
}

export function memoryRetrievalExplain(args: Omit<MemoryRetrievalExplainCommand, "cmd">): MemoryRetrievalExplainCommand {
  return { cmd: "memory_retrieval_explain", ...args };
}

/**
 * Answer the session's open question.
 */
export interface NoesisAnswerCommand {
  "cmd": "noesis_answer";
  "answer": AnswerInput;
  "idempotency_key"?: string | null;
  "question": string;
  "session": string;
}

export function noesisAnswer(args: Omit<NoesisAnswerCommand, "cmd">): NoesisAnswerCommand {
  return { cmd: "noesis_answer", ...args };
}

export interface NoesisCancelJobCommand {
  "cmd": "noesis_cancel_job";
  "idempotency_key"?: string | null;
  "job": string;
}

export function noesisCancelJob(args: Omit<NoesisCancelJobCommand, "cmd">): NoesisCancelJobCommand {
  return { cmd: "noesis_cancel_job", ...args };
}

/**
 * Write a subset of the `[noesis]` settings and answer with the new
 * status. Works while Noesis is disabled. Host only when the patch sets
 * `enabled`, a budget, billing or prompt overrides.
 */
export interface NoesisConfigureCommand {
  "cmd": "noesis_configure";
  "idempotency_key"?: string | null;
  "patch": NoesisSettingsPatch;
}

export function noesisConfigure(args: Omit<NoesisConfigureCommand, "cmd">): NoesisConfigureCommand {
  return { cmd: "noesis_configure", ...args };
}

/**
 * Record the operator's correction of a concept's state.
 */
export interface NoesisCorrectLearnerCommand {
  "cmd": "noesis_correct_learner";
  "concept": string;
  "idempotency_key"?: string | null;
  "reason": string;
  "state": ConceptStateKind;
}

export function noesisCorrectLearner(args: Omit<NoesisCorrectLearnerCommand, "cmd">): NoesisCorrectLearnerCommand {
  return { cmd: "noesis_correct_learner", ...args };
}

/**
 * Start a teaching or practice session. Answered with `noesis_session`.
 */
export interface NoesisCreateSessionCommand {
  "cmd": "noesis_create_session";
  "context"?: EvidenceBundle | null;
  "goal": string;
  "idempotency_key"?: string | null;
  "mode": SessionMode;
  "practice_targets"?: Array<string> | null;
}

export function noesisCreateSession(args: Omit<NoesisCreateSessionCommand, "cmd">): NoesisCreateSessionCommand {
  return { cmd: "noesis_create_session", ...args };
}

/**
 * Delete a proposition; its chains go with it.
 */
export interface NoesisDeletePropositionCommand {
  "cmd": "noesis_delete_proposition";
  "idempotency_key"?: string | null;
  "proposition": string;
}

export function noesisDeleteProposition(args: Omit<NoesisDeletePropositionCommand, "cmd">): NoesisDeletePropositionCommand {
  return { cmd: "noesis_delete_proposition", ...args };
}

export interface NoesisDeleteSessionCommand {
  "cmd": "noesis_delete_session";
  "graph": DeleteSessionGraph;
  "idempotency_key"?: string | null;
  "session": string;
}

export function noesisDeleteSession(args: Omit<NoesisDeleteSessionCommand, "cmd">): NoesisDeleteSessionCommand {
  return { cmd: "noesis_delete_session", ...args };
}

/**
 * Refused as a conflict while a job on the source runs.
 */
export interface NoesisDeleteSourceCommand {
  "cmd": "noesis_delete_source";
  "graph": DeleteSourceGraph;
  "idempotency_key"?: string | null;
  "source": string;
}

export function noesisDeleteSource(args: Omit<NoesisDeleteSourceCommand, "cmd">): NoesisDeleteSourceCommand {
  return { cmd: "noesis_delete_source", ...args };
}

/**
 * Extract the graph of exactly one of `session` and `source`.
 */
export interface NoesisExtractCommand {
  "cmd": "noesis_extract";
  "idempotency_key"?: string | null;
  "session"?: string | null;
  "source"?: string | null;
}

export function noesisExtract(args: Omit<NoesisExtractCommand, "cmd">): NoesisExtractCommand {
  return { cmd: "noesis_extract", ...args };
}

export interface NoesisGetSessionCommand {
  "cmd": "noesis_get_session";
  "session": string;
}

export function noesisGetSession(args: Omit<NoesisGetSessionCommand, "cmd">): NoesisGetSessionCommand {
  return { cmd: "noesis_get_session", ...args };
}

/**
 * Propositions, chains and clusters, filtered by text, session or
 * source. `limit` is 1 to 5000.
 */
export interface NoesisGraphCommand {
  "cmd": "noesis_graph";
  "limit"?: number | null;
  "query"?: string | null;
  "session"?: string | null;
  "source"?: string | null;
}

export function noesisGraph(args: Omit<NoesisGraphCommand, "cmd">): NoesisGraphCommand {
  return { cmd: "noesis_graph", ...args };
}

/**
 * Import an artifact as a source and extract it.
 */
export interface NoesisImportSourceCommand {
  "cmd": "noesis_import_source";
  "artifact": ArtifactId;
  "idempotency_key"?: string | null;
  "name": string;
}

export function noesisImportSource(args: Omit<NoesisImportSourceCommand, "cmd">): NoesisImportSourceCommand {
  return { cmd: "noesis_import_source", ...args };
}

export interface NoesisJobsCommand {
  "cmd": "noesis_jobs";
  "state"?: JobState | null;
}

export function noesisJobs(args: Omit<NoesisJobsCommand, "cmd">): NoesisJobsCommand {
  return { cmd: "noesis_jobs", ...args };
}

/**
 * Learner state of the named concepts, or of those matching `query`.
 * `limit` is 1 to 1000.
 */
export interface NoesisLearnerCommand {
  "cmd": "noesis_learner";
  "concepts"?: Array<string> | null;
  "limit"?: number | null;
  "query"?: string | null;
}

export function noesisLearner(args: Omit<NoesisLearnerCommand, "cmd">): NoesisLearnerCommand {
  return { cmd: "noesis_learner", ...args };
}

/**
 * A concept's evidence, newest first, from before the evidence id
 * `before`. `limit` is 1 to 500.
 */
export interface NoesisLearnerEvidenceCommand {
  "cmd": "noesis_learner_evidence";
  "before"?: number | null;
  "concept": string;
  "limit"?: number | null;
}

export function noesisLearnerEvidence(args: Omit<NoesisLearnerEvidenceCommand, "cmd">): NoesisLearnerEvidenceCommand {
  return { cmd: "noesis_learner_evidence", ...args };
}

/**
 * One page of sessions, from just after `after`. `limit` is 1 to 200.
 */
export interface NoesisListSessionsCommand {
  "cmd": "noesis_list_sessions";
  "after"?: string | null;
  "limit"?: number | null;
}

export function noesisListSessions(args: Omit<NoesisListSessionsCommand, "cmd">): NoesisListSessionsCommand {
  return { cmd: "noesis_list_sessions", ...args };
}

/**
 * Say something to the session's teacher. 1 to 20000 characters.
 */
export interface NoesisSendCommand {
  "cmd": "noesis_send";
  "idempotency_key"?: string | null;
  "session": string;
  "text": string;
}

export function noesisSend(args: Omit<NoesisSendCommand, "cmd">): NoesisSendCommand {
  return { cmd: "noesis_send", ...args };
}

/**
 * Noesis state: whether it is enabled, its settings as stored and as
 * effective, running jobs and today's spend. Needs `features::NOESIS`.
 */
export interface NoesisStatusCommand {
  "cmd": "noesis_status";
}

export function noesisStatus(): NoesisStatusCommand {
  return { cmd: "noesis_status" };
}

/**
 * Pause an agent: a durable flag, orthogonal to its status. With
 * `subtree` it reaches every live agent beneath it too (§3.1).
 */
export interface PauseCommand {
  "cmd": "pause";
  "agent": AgentId;
  "idempotency_key"?: string | null;
  "subtree"?: boolean;
}

export function pause(args: Omit<PauseCommand, "cmd">): PauseCommand {
  return { cmd: "pause", ...args };
}

/**
 * Keep a memory in front of the model, or stop.
 */
export interface PinMemoryCommand {
  "cmd": "pin_memory";
  "id": MemoryId;
  "pinned": boolean;
}

export function pinMemory(args: Omit<PinMemoryCommand, "cmd">): PinMemoryCommand {
  return { cmd: "pin_memory", ...args };
}

/**
 * Keep an otherwise silent connection alive under an idle timeout.
 * Answered with [`Reply::Ok`].
 */
export interface PingCommand {
  "cmd": "ping";
}

export function ping(): PingCommand {
  return { cmd: "ping" };
}

/**
 * Post a notification as the service `source`, or update the one its
 * `key` already names. Needs `features::NOTIFICATIONS`.
 */
export interface PostNotificationCommand {
  "cmd": "post_notification";
  "notification": NotificationPost;
  "source": string;
}

export function postNotification(args: Omit<PostNotificationCommand, "cmd">): PostNotificationCommand {
  return { cmd: "post_notification", ...args };
}

/**
 * Read-only retention decisions; never applies deletion or cleanup.
 */
export interface PreviewRetentionCommand {
  "cmd": "preview_retention";
}

export function previewRetention(): PreviewRetentionCommand {
  return { cmd: "preview_retention" };
}

/**
 * Every provider the registry knows, with its live health.
 */
export interface ProvidersCommand {
  "cmd": "providers";
}

export function providers(): ProvidersCommand {
  return { cmd: "providers" };
}

export interface PublishArtifactCommand {
  "cmd": "publish_artifact";
  "content_type": string;
  "name": string;
  "summary": string;
  "task": TaskId;
  "text": string;
}

export function publishArtifact(args: Omit<PublishArtifactCommand, "cmd">): PublishArtifactCommand {
  return { cmd: "publish_artifact", ...args };
}

/**
 * Bytes, base64-encoded, capped at `max_bytes`.
 */
export interface ReadArtifactCommand {
  "cmd": "read_artifact";
  "id": ArtifactId;
  "max_bytes": number;
}

export function readArtifact(args: Omit<ReadArtifactCommand, "cmd">): ReadArtifactCommand {
  return { cmd: "read_artifact", ...args };
}

/**
 * Record host evidence on a memory a task saw. Needs `memory_feedback`.
 */
export interface RecordMemoryFeedbackCommand {
  "cmd": "record_memory_feedback";
  "evidence": MemoryEvidence;
  "idempotency_key"?: string | null;
  "invocation"?: InvocationId | null;
  "memory": MemoryId;
  "reason": string;
  "task": TaskId;
  "verdict": MemoryVerdict;
}

export function recordMemoryFeedback(args: Omit<RecordMemoryFeedbackCommand, "cmd">): RecordMemoryFeedbackCommand {
  return { cmd: "record_memory_feedback", ...args };
}

/**
 * Refresh stale quota observations through explicitly enabled probe turns.
 */
export interface RefreshProviderUsageCommand {
  "cmd": "refresh_provider_usage";
  "provider"?: string | null;
}

export function refreshProviderUsage(args: Omit<RefreshProviderUsageCommand, "cmd">): RefreshProviderUsageCommand {
  return { cmd: "refresh_provider_usage", ...args };
}

/**
 * Reject a pending proposal with an explanation. Needs `memory_proposals`.
 */
export interface RejectMemoryCommand {
  "cmd": "reject_memory";
  "id": MemoryId;
  "reason": string;
}

export function rejectMemory(args: Omit<RejectMemoryCommand, "cmd">): RejectMemoryCommand {
  return { cmd: "reject_memory", ...args };
}

/**
 * One agent's turns as they happened, folded from the log.
 */
export interface ReplayCommand {
  "cmd": "replay";
  "agent": AgentId;
}

export function replay(args: Omit<ReplayCommand, "cmd">): ReplayCommand {
  return { cmd: "replay", ...args };
}

/**
 * Operator-only reply to one unanswered, unsuperseded question event.
 * A stale question is refused without delivering anything.
 */
export interface ReplyToQuestionCommand {
  "cmd": "reply_to_question";
  "content": string;
  "idempotency_key"?: string | null;
  "question_seq": number;
  "to": AgentId;
}

export function replyToQuestion(args: Omit<ReplyToQuestionCommand, "cmd">): ReplyToQuestionCommand {
  return { cmd: "reply_to_question", ...args };
}

export interface ResolveApprovalCommand {
  "cmd": "resolve_approval";
  "decision": ApprovalDecision;
  "id": ApprovalId;
  "idempotency_key"?: string | null;
  "note"?: string | null;
  "remember"?: RememberSpec | null;
}

export function resolveApproval(args: Omit<ResolveApprovalCommand, "cmd">): ResolveApprovalCommand {
  return { cmd: "resolve_approval", ...args };
}

/**
 * Resolve a notification by `id`, or by the `source` and `key` it was
 * posted under; exactly one of the two. Needs `features::NOTIFICATIONS`.
 */
export interface ResolveNotificationCommand {
  "cmd": "resolve_notification";
  "id"?: NotificationId | null;
  "key"?: string | null;
  "source"?: string | null;
}

export function resolveNotification(args: Omit<ResolveNotificationCommand, "cmd">): ResolveNotificationCommand {
  return { cmd: "resolve_notification", ...args };
}

/**
 * Ask what became of a keyed command whose acknowledgement was lost.
 * Read-only, within the caller's own keys; needs `command`'s standing.
 */
export interface ResolveSubmissionCommand {
  "cmd": "resolve_submission";
  "command": Record<string, unknown>;
  "horizon"?: string | null;
}

export function resolveSubmission(args: Omit<ResolveSubmissionCommand, "cmd">): ResolveSubmissionCommand {
  return { cmd: "resolve_submission", ...args };
}

/**
 * Recreate a failed or cancelled agent from its last checkpoint, as a
 * new agent linked to it.
 */
export interface RestartCommand {
  "cmd": "restart";
  "agent": AgentId;
  "idempotency_key"?: string | null;
}

export function restart(args: Omit<RestartCommand, "cmd">): RestartCommand {
  return { cmd: "restart", ...args };
}

/**
 * Put a forgotten memory back. The inverse of `ForgetMemory`, so a
 * mis-keyed curation has a way back.
 */
export interface RestoreMemoryCommand {
  "cmd": "restore_memory";
  "id": MemoryId;
}

export function restoreMemory(args: Omit<RestoreMemoryCommand, "cmd">): RestoreMemoryCommand {
  return { cmd: "restore_memory", ...args };
}

/**
 * Lift a pause, or clear a wait or an inspection the operator owns.
 * With `subtree` it lifts the pause across the subtree, and only that.
 */
export interface ResumeCommand {
  "cmd": "resume";
  "agent": AgentId;
  "idempotency_key"?: string | null;
  "subtree"?: boolean;
}

export function resume(args: Omit<ResumeCommand, "cmd">): ResumeCommand {
  return { cmd: "resume", ...args };
}

/**
 * Withdraw evidence, preserving the record. Needs `memory_feedback`.
 */
export interface RetractMemoryFeedbackCommand {
  "cmd": "retract_memory_feedback";
  "feedback": MemoryFeedbackId;
  "reason": string;
}

export function retractMemoryFeedback(args: Omit<RetractMemoryFeedbackCommand, "cmd">): RetractMemoryFeedbackCommand {
  return { cmd: "retract_memory_feedback", ...args };
}

export interface RevokeApprovalRuleCommand {
  "cmd": "revoke_approval_rule";
  "id": ApprovalRuleId;
  "reason"?: string | null;
}

export function revokeApprovalRule(args: Omit<RevokeApprovalRuleCommand, "cmd">): RevokeApprovalRuleCommand {
  return { cmd: "revoke_approval_rule", ...args };
}

/**
 * Revoke a device and close its connections and subscriptions at once.
 * Host only.
 */
export interface RevokeDeviceCommand {
  "cmd": "revoke_device";
  "id": DeviceId;
  "reason"?: string | null;
}

export function revokeDevice(args: Omit<RevokeDeviceCommand, "cmd">): RevokeDeviceCommand {
  return { cmd: "revoke_device", ...args };
}

export interface ScheduleHistoryCommand {
  "cmd": "schedule_history";
  "id": ScheduleId;
  "limit": number;
}

export function scheduleHistory(args: Omit<ScheduleHistoryCommand, "cmd">): ScheduleHistoryCommand {
  return { cmd: "schedule_history", ...args };
}

/**
 * Deliver a user message to an agent's inbox; wakes it if waiting.
 * With `subtree`, the message is fanned out to every live agent beneath
 * it too, each with its own inbox item and events (§3.5).
 */
export interface SendMessageCommand {
  "cmd": "send_message";
  "attachments"?: Array<ImageAttachment>;
  "content": string;
  "idempotency_key"?: string | null;
  "subtree"?: boolean;
  "to": AgentId;
}

export function sendMessage(args: Omit<SendMessageCommand, "cmd">): SendMessageCommand {
  return { cmd: "send_message", ...args };
}

/**
 * Turn approvals off (yolo) or back on for an agent and, with
 * `subtree`, every live agent beneath it. A separate command
 * from `UpdateAgent` because it carries the subtree reach and the
 * never-widen-past-the-parent rule.
 */
export interface SetApprovalsCommand {
  "cmd": "set_approvals";
  "agent": AgentId;
  "idempotency_key"?: string | null;
  "policy": ApprovalPolicy;
  "subtree"?: boolean;
}

export function setApprovals(args: Omit<SetApprovalsCommand, "cmd">): SetApprovalsCommand {
  return { cmd: "set_approvals", ...args };
}

export interface ShutdownCommand {
  "cmd": "shutdown";
}

export function shutdown(): ShutdownCommand {
  return { cmd: "shutdown" };
}

/**
 * Open a pairing and mint its single-use code. Host only.
 */
export interface StartPairingCommand {
  "cmd": "start_pairing";
  "role"?: DeviceRole;
}

export function startPairing(args: Omit<StartPairingCommand, "cmd">): StartPairingCommand {
  return { cmd: "start_pairing", ...args };
}

export interface StatusCommand {
  "cmd": "status";
}

export function status(): StatusCommand {
  return { cmd: "status" };
}

/**
 * Host-only filesystem maintenance. Preview never deletes files.
 */
export interface StorageGcCommand {
  "cmd": "storage_gc";
  "dry_run": boolean;
  "prune_build_output"?: boolean;
}

export function storageGc(args: Omit<StorageGcCommand, "cmd">): StorageGcCommand {
  return { cmd: "storage_gc", ...args };
}

/**
 * Bounded filesystem inventory and eligibility preview.
 */
export interface StorageStatusCommand {
  "cmd": "storage_status";
}

export function storageStatus(): StorageStatusCommand {
  return { cmd: "storage_status" };
}

/**
 * Stream events with `seq > after` (0 replays everything, `None` starts live).
 */
export interface SubscribeCommand {
  "cmd": "subscribe";
  "after"?: number | null;
  "deltas": boolean;
  "query": EventQuery;
}

export function subscribe(args: Omit<SubscribeCommand, "cmd">): SubscribeCommand {
  return { cmd: "subscribe", ...args };
}

/**
 * Store the calling device's push subscription, replacing any earlier
 * one. Only a paired device can call it.
 */
export interface SubscribePushCommand {
  "cmd": "subscribe_push";
  "kinds"?: Array<AttentionKind>;
  "subscription": PushSubscription;
}

export function subscribePush(args: Omit<SubscribePushCommand, "cmd">): SubscribePushCommand {
  return { cmd: "subscribe_push", ...args };
}

/**
 * Read a repository's committed notes into memory scoped to it.
 *
 * The scope is derived from the root, never named by the caller: reach
 * to the socket is the whole authorization boundary (ARCHITECTURE §16),
 * and a caller that could name both would be able to read notes out of a
 * directory it just wrote and admit them into any repository's memory,
 * attributed to the person.
 */
export interface SyncRepositoryKnowledgeCommand {
  "cmd": "sync_repository_knowledge";
  "root"?: string | null;
}

export function syncRepositoryKnowledge(args: Omit<SyncRepositoryKnowledgeCommand, "cmd">): SyncRepositoryKnowledgeCommand {
  return { cmd: "sync_repository_knowledge", ...args };
}

/**
 * Send the calling device one `test` notification.
 */
export interface TestPushCommand {
  "cmd": "test_push";
}

export function testPush(): TestPushCommand {
  return { cmd: "test_push" };
}

/**
 * Deliver an external trigger (a webhook, a timer, a CI result) to an
 * agent's inbox. It wakes a waiting agent like a person's message.
 */
export interface TriggerCommand {
  "cmd": "trigger";
  "agent": AgentId;
  "idempotency_key"?: string | null;
  "payload"?: unknown;
  "source": string;
}

export function trigger(args: Omit<TriggerCommand, "cmd">): TriggerCommand {
  return { cmd: "trigger", ...args };
}

export interface UnsubscribeCommand {
  "cmd": "unsubscribe";
  "id": number;
}

export function unsubscribe(args: Omit<UnsubscribeCommand, "cmd">): UnsubscribeCommand {
  return { cmd: "unsubscribe", ...args };
}

/**
 * Forget the calling device's push subscription.
 */
export interface UnsubscribePushCommand {
  "cmd": "unsubscribe_push";
}

export function unsubscribePush(): UnsubscribePushCommand {
  return { cmd: "unsubscribe_push" };
}

export interface UpdateAgentCommand {
  "cmd": "update_agent";
  "id": AgentId;
  "idempotency_key"?: string | null;
  "patch": AgentPatch;
  "subtree"?: boolean;
}

export function updateAgent(args: Omit<UpdateAgentCommand, "cmd">): UpdateAgentCommand {
  return { cmd: "update_agent", ...args };
}

/**
 * Rename a device or change its role. A role change closes its live
 * connections. Host only.
 */
export interface UpdateDeviceCommand {
  "cmd": "update_device";
  "id": DeviceId;
  "name"?: string | null;
  "role"?: DeviceRole | null;
}

export function updateDevice(args: Omit<UpdateDeviceCommand, "cmd">): UpdateDeviceCommand {
  return { cmd: "update_device", ...args };
}

export interface UpdateScheduleCommand {
  "cmd": "update_schedule";
  "action": ScheduleAction;
  "id": ScheduleId;
  "idempotency_key"?: string | null;
}

export function updateSchedule(args: Omit<UpdateScheduleCommand, "cmd">): UpdateScheduleCommand {
  return { cmd: "update_schedule", ...args };
}

/**
 * Spend summed from `invocation.completed` events, grouped by `by`, in
 * `since..until` (completion time, `until` exclusive). `limit` is 1 to
 * 1000 rows, 1000 when absent. Needs `features::USAGE_BREAKDOWN`.
 */
export interface UsageBreakdownCommand {
  "cmd": "usage_breakdown";
  "by": UsageGroup;
  "limit"?: number | null;
  "scope"?: UsageScope;
  "since"?: string | null;
  "until"?: string | null;
}

export function usageBreakdown(args: Omit<UsageBreakdownCommand, "cmd">): UsageBreakdownCommand {
  return { cmd: "usage_breakdown", ...args };
}

/**
 * Whether an agent can be started in `path`, and its canonical form.
 */
export interface ValidateWorkspaceCommand {
  "cmd": "validate_workspace";
  "path": string;
}

export function validateWorkspace(args: Omit<ValidateWorkspaceCommand, "cmd">): ValidateWorkspaceCommand {
  return { cmd: "validate_workspace", ...args };
}

/**
 * Bring a failed agent back in place, with its history and context:
 * it becomes Ready and reachable again under the same id. Additive in
 * protocol 8; older daemons refuse the tag as unknown.
 */
export interface WakeCommand {
  "cmd": "wake";
  "agent": AgentId;
}

export function wake(args: Omit<WakeCommand, "cmd">): WakeCommand {
  return { cmd: "wake", ...args };
}

/**
 * Files, diffstat, commits and a capped patch of an agent's work.
 */
export interface WorkspaceDiffCommand {
  "cmd": "workspace_diff";
  "agent": AgentId;
}

export function workspaceDiff(args: Omit<WorkspaceDiffCommand, "cmd">): WorkspaceDiffCommand {
  return { cmd: "workspace_diff", ...args };
}

/**
 * Throw away an agent's isolated workspace. Refused while an agent using
 * it is live unless `force`, which does not stop that agent.
 */
export interface WorkspaceDiscardCommand {
  "cmd": "workspace_discard";
  "agent": AgentId;
  "force"?: boolean;
}

export function workspaceDiscard(args: Omit<WorkspaceDiscardCommand, "cmd">): WorkspaceDiscardCommand {
  return { cmd: "workspace_discard", ...args };
}

/**
 * Land a finished child's isolated workspace in the source it came
 * from, through the same conflict check as the `integrate_workspace` tool.
 */
export interface WorkspaceIntegrateCommand {
  "cmd": "workspace_integrate";
  "agent": AgentId;
}

export function workspaceIntegrate(args: Omit<WorkspaceIntegrateCommand, "cmd">): WorkspaceIntegrateCommand {
  return { cmd: "workspace_integrate", ...args };
}

/** Every command the daemon accepts. */
export type Command =
  | AcceptMemoryCommand
  | AddApprovalRuleCommand
  | AliasRepositoryCommand
  | AnswerNotificationCommand
  | AppendNoteCommand
  | ArchiveCommand
  | AuthStatusCommand
  | CancelCommand
  | CancelPairingCommand
  | CompactCommand
  | CompletePathCommand
  | ConfigureGithubWatcherCommand
  | ConfigureRemoteCommand
  | ConfirmPairingCommand
  | ConsolidateMemoriesCommand
  | CreateAgentCommand
  | CreateMemoryCommand
  | CreateScheduleCommand
  | CreateTaskCommand
  | EngineToolCommand
  | EngineToolsCommand
  | ExportRunCommand
  | ForgetMemoryCommand
  | GetAgentCommand
  | GetArtifactCommand
  | GetAttentionCommand
  | GetCheckpointCommand
  | GetEventsCommand
  | GetGitSummaryCommand
  | GetGithubWatcherCommand
  | GetInboxCommand
  | GetInvocationCommand
  | GetNotificationCommand
  | GetPairingCommand
  | GetPushConfigCommand
  | GetRemoteCommand
  | GetRunReportCommand
  | GetScheduleCommand
  | GetSnapshotCommand
  | GetTaskCommand
  | GetTranscriptCommand
  | GetTranscriptPageCommand
  | GetTreeCommand
  | HelloCommand
  | ImportRunCommand
  | InterruptCommand
  | LatestCheckpointCommand
  | ListAgentsCommand
  | ListAgentsPageCommand
  | ListApprovalRulesCommand
  | ListApprovalsCommand
  | ListArtifactsCommand
  | ListDevicesCommand
  | ListInvocationsCommand
  | ListModelsCommand
  | ListNotificationsCommand
  | ListRepositoryAliasesCommand
  | ListRunsCommand
  | ListSchedulesCommand
  | ListTasksCommand
  | ListViewModesCommand
  | ListWorkspacesCommand
  | MarkNotificationReadCommand
  | McpServersCommand
  | MemoriesCommand
  | MemoriesInContextCommand
  | MemoryExportCommand
  | MemoryFeedbackCommand
  | MemoryImportCommand
  | MemoryMigrationStatusCommand
  | MemoryRetrievalExplainCommand
  | NoesisAnswerCommand
  | NoesisCancelJobCommand
  | NoesisConfigureCommand
  | NoesisCorrectLearnerCommand
  | NoesisCreateSessionCommand
  | NoesisDeletePropositionCommand
  | NoesisDeleteSessionCommand
  | NoesisDeleteSourceCommand
  | NoesisExtractCommand
  | NoesisGetSessionCommand
  | NoesisGraphCommand
  | NoesisImportSourceCommand
  | NoesisJobsCommand
  | NoesisLearnerCommand
  | NoesisLearnerEvidenceCommand
  | NoesisListSessionsCommand
  | NoesisSendCommand
  | NoesisStatusCommand
  | PauseCommand
  | PinMemoryCommand
  | PingCommand
  | PostNotificationCommand
  | PreviewRetentionCommand
  | ProvidersCommand
  | PublishArtifactCommand
  | ReadArtifactCommand
  | RecordMemoryFeedbackCommand
  | RefreshProviderUsageCommand
  | RejectMemoryCommand
  | ReplayCommand
  | ReplyToQuestionCommand
  | ResolveApprovalCommand
  | ResolveNotificationCommand
  | ResolveSubmissionCommand
  | RestartCommand
  | RestoreMemoryCommand
  | ResumeCommand
  | RetractMemoryFeedbackCommand
  | RevokeApprovalRuleCommand
  | RevokeDeviceCommand
  | ScheduleHistoryCommand
  | SendMessageCommand
  | SetApprovalsCommand
  | ShutdownCommand
  | StartPairingCommand
  | StatusCommand
  | StorageGcCommand
  | StorageStatusCommand
  | SubscribeCommand
  | SubscribePushCommand
  | SyncRepositoryKnowledgeCommand
  | TestPushCommand
  | TriggerCommand
  | UnsubscribeCommand
  | UnsubscribePushCommand
  | UpdateAgentCommand
  | UpdateDeviceCommand
  | UpdateScheduleCommand
  | UsageBreakdownCommand
  | ValidateWorkspaceCommand
  | WakeCommand
  | WorkspaceDiffCommand
  | WorkspaceDiscardCommand
  | WorkspaceIntegrateCommand;
