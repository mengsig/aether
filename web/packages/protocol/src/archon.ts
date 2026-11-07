// The Archon daemon protocol as TypeScript types and command builders.
// Generated from web/schemas/archon.protocol.schema.json by web/scripts/generate-protocol.ts.
// Do not edit: regenerate with `npm run generate` in web/.
// Schema SHA-256: 249cefb63f214b7a4d3c7129f81e0eb48345b37a73ff8db50a043891bb19d1c9

export const PROTOCOL_VERSION = 24;

export type AetherAgentId = string;

/**
 * The outcome of `storage_gc`, which Aether runs to delete the workspaces it
 * delegated to agents whose worktrees gc removed. A failure never fails gc.
 */
export type AetherGc = "ran" | Record<string, unknown>;

/**
 * Tagged by `state`.
 */
export interface AetherLinkStatusConnected {
  "daemon_version": string;
  "instance_id": string;
  "protocol_version": number;
  "state": "connected";
}

export interface AetherLinkStatusDisconnected {
  "error": string;
  "retry_at"?: string | null;
  "state": "disconnected";
}

export type AetherLinkStatus = AetherLinkStatusConnected | AetherLinkStatusDisconnected;

export type AetherMemoryFeedbackId = string;

/**
 * An Aether memory id, checked exactly as Aether's schema states it
 * (`mem_` and an upper-case ULID): the operator types it for a forget.
 */
export type AetherMemoryId = string;

/**
 * A pending Aether `ask_user` question: answered with `reply_to_question`.
 */
export interface AetherQuestionRef {
  "agent": AetherAgentId;
  "seq": number;
}

export type AetherRunId = string;

export type AetherTaskId = string;

/**
 * The GitHub logins that alone may dispatch for one repository: non-empty,
 * at most [`MAX_ALLOWED_USERS`], lowercased and de-duplicated in first-seen
 * order, as Aether stores them.
 */
export type AllowedUsers = Array<string>;

/**
 * The surface an answer came in through; the audit mark of a feed answer.
 */
export type AnswerVia = "archon" | "chat_command" | "chat_reply" | "notification_center";

/**
 * One file of an item's artifact store, named relative to it.
 */
export interface ArtifactFile {
  "bytes": number;
  "name": string;
}

/**
 * An item's artifact store: where it is and what it holds.
 */
export interface ArtifactListing {
  "dir": string;
  "files": Array<ArtifactFile>;
  "item": QueueItemId;
  "omitted": number;
  "published"?: Array<ArtifactFile>;
  "total_bytes": number;
}

/**
 * A page of one artifact's text.
 */
export interface ArtifactPage {
  "item": QueueItemId;
  "name": string;
  "next_offset"?: number | null;
  "offset": number;
  "path": string;
  "text": string;
  "total_bytes": number;
}

/**
 * What a delivered pull request says of Archon and of the tools that made
 * it; only the operator sets it, in the registry.
 */
export type Attribution = "none" | "item-id";

/**
 * What the feedback froze about the run when it was recorded.
 */
export interface Attribution2 {
  "context"?: RouteContext | null;
  "credit": Credit;
  "delivery_commit"?: string | null;
  "pipeline": string;
  "repo"?: RepoId | null;
}

/**
 * Every knob set. The default is standard review, no proposal or report,
 * manual merge and guarded tools.
 */
export interface Autonomy {
  "merge"?: MergeMode;
  "proposal"?: ProposalMode;
  "report"?: ReportMode;
  "review"?: ReviewLevel;
  "tools"?: ToolsMode;
}

/**
 * Some knobs; unset ones come from elsewhere. Used for configured
 * defaults and ceilings, a repository's registry ceiling, and enqueues.
 */
export interface AutonomyPatch {
  "merge"?: MergeMode | null;
  "proposal"?: ProposalMode | null;
  "report"?: ReportMode | null;
  "review"?: ReviewLevel | null;
  "tools"?: ToolsMode | null;
}

/**
 * The knobs an item starts from and the ceiling it must stay within, for
 * one repository (or none).
 */
export interface AutonomyPolicy {
  "ceiling": Autonomy;
  "default": Autonomy;
}

/**
 * What `archon autonomy` and the `autonomy_policy` tool show.
 */
export interface AutonomyView {
  "directives": Array<Directive>;
  "limits"?: Array<StructuralLimit>;
  "merge_granted"?: Array<string>;
  "policy": AutonomyPolicy;
  "repo"?: string | null;
}

/**
 * A measured value past its limit.
 */
export interface Breach {
  "limit": number;
  "measured": number;
}

/**
 * Concurrency and spend limits the dispatcher enforces. Zero means
 * "no limit" for every field, matching the config file's convention.
 */
export interface Caps {
  "budget_usd_daily": number;
  "budget_usd_per_item": number;
  "budget_usd_total": number;
  "budgets_enabled"?: boolean;
  "max_active_runs": number;
  "max_parallel_agents": number;
  "max_per_pipeline": Record<string, number>;
  "max_per_repo": number;
}

/**
 * A review that covers a later head than the commit it passed: every commit
 * since merges in only what the pull request's base holds.
 */
export interface CarriedReview {
  "reviewed": string;
  "why": string;
}

export interface CatalogListing {
  "pipelines": Array<PipelineSummary>;
  "problems": Array<PipelineProblem>;
}

/**
 * One committed state change. `seq` is gapless and written in the same
 * transaction as the state it describes; the subject's fields sit beside it.
 */
export interface Change {
  "at": string;
  "seq": number;
}

/**
 * The check tiers: `fast` runs in every item before it commits, for the
 * units the change affects where it names `{packages}` or
 * `{packages:<flag>}` ([`super::affected`]); `full` is the whole suite,
 * which items never run; `release` runs once a promotion merged the
 * integration branch into the release branch.
 */
export interface Checks {
  "fast"?: string | null;
  "full"?: string | null;
  "release"?: string | null;
}

/**
 * How a delivered pull request's checks that never start (billing, no
 * runner) reach the orchestrator.
 */
export type ChecksMode = "watched" | "waived";

export type ChecksState = "pending" | "success" | "failure" | "neutral";

/**
 * A pull request's checks, rolled up.
 */
export interface ChecksSummary {
  "failed": number;
  "state": ChecksState;
  "total": number;
}

/**
 * One choice's routing samples in the feedback's cohort, read now.
 */
export interface ChoiceStanding {
  "choice": ModelChoice;
  "passed": number;
  "runs": number;
}

/**
 * A patch to an optional setting. Explicit on the wire (`"keep"`,
 * `"clear"`, `{"set":v}`) because JSON collapses `Option<Option<T>>`.
 */
export type Clearable = "keep" | "clear" | Record<string, unknown>;

/**
 * A patch to an optional setting. Explicit on the wire (`"keep"`,
 * `"clear"`, `{"set":v}`) because JSON collapses `Option<Option<T>>`.
 */
export type Clearable2 = "keep" | "clear" | Record<string, unknown>;

/**
 * Everything a client can ask `archond`. MCP tools map 1:1 onto a subset
 * (ARCHITECTURE §11).
 */
export type ClientFrame = { "req": number } & Command;

/**
 * Who is on the other end. Guards against mistakes, not attacks: the
 * socket's file mode is the security boundary.
 */
export type ClientKind = "archon-cli" | "archon-mcp" | "archon-bench" | "archon-web";

export type CorrectionCause = "quality" | "scope" | "infrastructure" | "budget";

/**
 * Money in one group. Classification does not change budget accounting:
 * `counted_usd` includes raw Aether `cost_usd` regardless of its basis, plus
 * raw `estimated_cost_usd` when `limits.count_estimated_cost` is on.
 */
export interface CostAmounts {
  "cost_usd": number;
  "counted_usd": number;
  "estimated_cost_usd": number;
  "unclassified_cost_usd"?: number;
}

/**
 * What a report sums spend by.
 */
export type CostGroup = "day" | "repo" | "pipeline" | "model" | "item" | "stage" | "provider";

/**
 * `Request::CostReport`: spend over a range of days, grouped by `by`.
 */
export interface CostQuery {
  "by"?: CostGroup;
  "since"?: DayBound | null;
  "until"?: DayBound | null;
}

/**
 * `Reply::CostReport`. Days in order for `by = day`; otherwise the most
 * counted first. The rows add up to `total`.
 */
export interface CostReport {
  "by": CostGroup;
  "estimates_counted": boolean;
  "range": DayRange;
  "rows": Array<CostRow>;
  "total": CostAmounts;
}

/**
 * Money in one group. Classification does not change budget accounting:
 * `counted_usd` includes raw Aether `cost_usd` regardless of its basis, plus
 * raw `estimated_cost_usd` when `limits.count_estimated_cost` is on.
 */
export interface CostRow {
  "cost_usd": number;
  "counted_usd": number;
  "estimated_cost_usd": number;
  "key"?: string | null;
  "unclassified_cost_usd"?: number;
}

/**
 * Tagged by `credit`.
 */
/**
 * The one completed model choice that produced the stage.
 */
export interface CreditModel {
  "choice": ModelChoice;
  "credit": "model";
  "stage_run": StageRunId;
}

/**
 * No model is credited: the producing choice is ambiguous or unknown.
 */
export interface CreditUnavailable {
  "credit": "unavailable";
  "why": string;
}

export type Credit = CreditModel | CreditUnavailable;

export type CurationState = "applied" | "failed" | "pending" | "restored";

export interface DaemonStatus {
  "active_runs": number;
  "aether": AetherLinkStatus;
  "autonomy": AutonomyPolicy;
  "blocked_items": number;
  "caps": Caps;
  "daemon_version": string;
  "directives": Array<Directive>;
  "disk"?: DiskStatus | null;
  "github"?: GithubWatch | null;
  "github_outbox"?: Array<GithubOutboxPending>;
  "github_outbox_pending"?: number;
  "health"?: StatusHealth | null;
  "held_news"?: number;
  "held_news_due"?: boolean;
  "host_load"?: HostLoadStatus | null;
  "integration"?: Array<IntegrationStatus>;
  "lavish_available": boolean;
  "mcp"?: McpStatus | null;
  "merge_granted"?: Array<string>;
  "merges_waiting"?: Array<MergeRequest>;
  "message_caps"?: Array<MessageCap>;
  "open_gates": number;
  "open_questions": number;
  "orchestrator"?: OrchestratorInfo | null;
  "pause": PauseState;
  "pid": number;
  "queued_items": number;
  "quota"?: QuotaStatus | null;
  "recovery"?: RecoveryProgress | null;
  "running_agents": number;
  "spend_today_usd": number;
  "spend_total_usd": number;
  "stage_warnings"?: Array<StageWarning>;
  "started_at": string;
  "upgrade"?: string | null;
  "versions"?: Versions | null;
  "waiting_items"?: Array<ItemSummary>;
  "waiting_next_offset"?: number | null;
  "waiting_total"?: number;
}

/**
 * One end of a report's range: a local calendar date, or a number of
 * whole days back from today.
 */
export type DayBound = Record<string, unknown>;

/**
 * Local calendar days, both ends included; never empty.
 */
export interface DayRange {
  "since": string;
  "until": string;
}

/**
 * An agent a stage agent delegated to, itself or through another.
 */
export interface DelegatedAgent {
  "agent": AetherAgentId;
  "cost_usd": number;
  "effort"?: Effort | null;
  "model": ModelRef;
  "parent": AetherAgentId;
}

/**
 * What a paired device may do: an operator runs the queue, an observer
 * only reads it.
 */
export type DeviceRole = "operator" | "observer";

/**
 * A paired device, as the operator sees it. Never carries its credential.
 */
export interface DeviceView {
  "id": string;
  "last_seen_at"?: string | null;
  "name": string;
  "paired_at": string;
  "role"?: DeviceRole;
  "sessions": number;
}

export type DiffSize = "small" | "medium" | "large";

/**
 * A standing operator instruction ("fully autonomous today") that steers
 * the orchestrator's knob choices within the ceiling; it never raises it.
 */
export interface Directive {
  "expires_at"?: string | null;
  "id": DirectiveId;
  "set_at": string;
  "source": DirectiveSource;
  "text": string;
}

export type DirectiveId = string;

/**
 * Who recorded a directive: the operator from the CLI, or the orchestrator
 * relaying what the operator said in chat.
 */
export type DirectiveSource = "operator" | "orchestrator";

/**
 * `DaemonStatus::disk`: the fuller of the filesystems holding the data
 * dir and the worktree root (ARCHITECTURE §7.3).
 */
export interface DiskStatus {
  "disk_full"?: string | null;
  "error"?: string | null;
  "free_bytes"?: number | null;
  "gc"?: GcStatus | null;
  "held": boolean;
  "min_free_bytes": number;
  "path": string;
  "standby"?: string | null;
  "storage"?: Array<StorageUsage>;
}

/**
 * The values in force for one scope, and where each came from.
 */
export interface EffectiveSettings {
  "autonomy": AutonomyPolicy;
  "caps": Caps;
  "checks"?: ChecksMode;
  "default_pipeline"?: string | null;
  "sources": Record<string, SettingSource>;
}

/**
 * Aether's effort scale; the wire spelling is snake_case (`x_high`).
 */
export type Effort = "low" | "medium" | "high" | "x_high" | "max" | "xhigh";

export interface EnqueueInitiative {
  "atomic"?: boolean;
  "contract": string;
  "idempotency_key"?: string | null;
  "members": Array<InitiativeMember>;
  "title": string;
}

export interface EnqueueRequest {
  "autonomy"?: AutonomyPatch;
  "brief": string;
  "budget_usd"?: number | null;
  "depends_on"?: Array<QueueItemId>;
  "idempotency_key"?: string | null;
  "model_plan"?: ModelPlanDraft;
  "model_reasons"?: Record<string, string> | null;
  "onto"?: OntoTarget | null;
  "origin"?: GithubThread | null;
  "pipeline"?: string | null;
  "priority"?: Priority;
  "repo"?: RepoRef | null;
  "stage_timeouts"?: Record<string, number>;
  "tags"?: Array<string>;
  "title": string;
}

export type ErrorCode = "incompatible" | "forbidden" | "not_found" | "invalid_argument" | "invalid_transition" | "policy" | "unavailable" | "internal" | "recovering" | Record<string, unknown>;

export interface ErrorReply {
  "code": ErrorCode;
  "message": string;
}

/**
 * One line of the operator chat. Persisted so `archon chat` can replay.
 * Tagged `entry` because the wrapped `Notification` already has a `kind`.
 * Tagged by `entry`.
 */
/**
 * The daemon observed an authenticated operator command.
 */
export interface FeedEntryAuthority {
  "action": string;
  "after": unknown;
  "before": unknown;
  "entry": "authority";
  "evidence": OperatorMessage;
  "result": string;
}

export interface FeedEntryOperator {
  "entry": "operator";
  "images"?: Array<ImageRef>;
  "text": string;
}

/**
 * Final text of an orchestrator turn that answered the operator.
 */
export interface FeedEntryOrchestrator {
  "entry": "orchestrator";
  "text": string;
}

export interface FeedEntryNotification {
  "at": string;
  "entry": "notification";
  "id": NotificationId;
  "item"?: QueueItemId | null;
  "kind": NotificationKind;
  "run"?: PipelineRunId | null;
  "text": string;
}

export interface FeedEntryQuestion {
  "aether"?: AetherQuestionRef | null;
  "answer"?: string | null;
  "answered_at"?: string | null;
  "asked_at": string;
  "asked_by"?: AetherAgentId | null;
  "entry": "question";
  "id": QuestionId;
  "item"?: QueueItemId | null;
  "number"?: number;
  "options": Array<string>;
  "text": string;
}

export interface FeedEntryAnswer {
  "entry": "answer";
  "question": QuestionId;
  "text": string;
  "via"?: AnswerVia;
}

/**
 * A report published for review (Lavish URL or plain file path).
 */
export interface FeedEntryReport {
  "entry": "report";
  "item"?: QueueItemId | null;
  "location": string;
  "report": ReportId;
  "title": string;
}

/**
 * Daemon-level notices: start/stop, orchestrator restarted, caps hit.
 */
export interface FeedEntrySystem {
  "entry": "system";
  "text": string;
}

export type FeedEntry =
  | FeedEntryAuthority
  | FeedEntryOperator
  | FeedEntryOrchestrator
  | FeedEntryNotification
  | FeedEntryQuestion
  | FeedEntryAnswer
  | FeedEntryReport
  | FeedEntrySystem;

export interface FeedItem {
  "at": string;
  "entry": FeedEntry;
  "id": FeedItemId;
  "seq": number;
}

export type FeedItemId = string;

export type FeedbackId = string;

/**
 * The reply of `learning correct`, `undo`, `restore-memory` and `retry-memory`.
 */
export interface FeedbackReceipt {
  "replayed": boolean;
  "view": FeedbackView;
}

/**
 * A feedback event with what it does now.
 */
export interface FeedbackView {
  "corrected"?: ChoiceStanding | null;
  "feedback": OperatorFeedback;
  "frozen_explanation"?: string | null;
  "memory"?: MemoryView | null;
  "memory_error"?: string | null;
  "memory_feedback"?: MemoryFeedbackCounts;
  "penalizing": boolean;
  "selected"?: ChoiceStanding | null;
}

export type Gate = "none" | "all_pass" | "majority" | "any";

/**
 * The operator's answer to a gate.
 * Tagged by `decision`.
 */
export interface GateDecisionApprove {
  "decision": "approve";
  "note"?: string | null;
}

/**
 * Loop back with `feedback` in the prompt.
 */
export interface GateDecisionRevise {
  "decision": "revise";
  "feedback": string;
}

/**
 * Cancel the item.
 */
export interface GateDecisionReject {
  "decision": "reject";
  "reason": string;
}

export type GateDecision = GateDecisionApprove | GateDecisionRevise | GateDecisionReject;

/**
 * What an operator gate asks the operator to judge.
 */
export type GateSubject = "proposal" | "report";

/**
 * What one gc pass did (ARCHITECTURE §13.2). Paths are real paths.
 */
export interface GcReport {
  "aether_gc"?: AetherGc | null;
  "artifacts"?: Array<Reclaimed>;
  "bytes_freed": number;
  "caches"?: Array<Reclaimed>;
  "failed": Array<Array<unknown>>;
  "item_caches"?: Array<Reclaimed>;
  "pressure"?: boolean;
  "reclaimed": Array<Reclaimed>;
  "removed": Array<string>;
  "reports"?: Array<Reclaimed>;
  "skipped": Array<Array<unknown>>;
  "would_delete"?: Array<WouldDelete>;
}

/**
 * `DiskStatus::gc`: the last pass that freed anything.
 */
export interface GcStatus {
  "at": string;
  "bytes_freed": number;
  "headline": string;
  "pressure": boolean;
}

/**
 * A github.com login whose `gh` token every `gh` call and push for a
 * repository carries; only the operator sets it, in the registry.
 */
export type GhUser = string;

export type GithubKind = "issue" | "pull_request";

export interface GithubLink {
  "created_at": string;
  "item": QueueItemId;
  "ref": GithubRef;
  "relation": LinkRelation;
  "source": LinkSource;
}

/**
 * An issue or pull request as last read from GitHub. Its text is
 * untrusted and grants no authority.
 */
export interface GithubObject {
  "author"?: string | null;
  "checks"?: ChecksSummary | null;
  "draft"?: boolean | null;
  "fetched_at": string;
  "labels": Array<string>;
  "ref": GithubRef;
  "review_decision"?: ReviewDecision | null;
  "state": GithubObjectState;
  "state_reason"?: string | null;
  "title": string;
  "updated_at": string;
}

export type GithubObjectState = "open" | "closed" | "merged";

/**
 * A GitHub comment Archon recorded but has not seen posted: it is retried,
 * at most once on GitHub, and never dropped (`DaemonStatus::github_outbox`).
 */
export interface GithubOutboxPending {
  "attempts": number;
  "created_at": string;
  "last_error"?: string | null;
  "next_attempt_at": string;
  "purpose": string;
  "target": string;
  "token": string;
}

/**
 * An issue or pull request on a GitHub host.
 */
export interface GithubRef {
  "host": string;
  "kind": GithubKind;
  "number": number;
  "repo": string;
}

/**
 * A GitHub.com `owner/name`, lowercased as Aether stores it.
 */
export type GithubRepo = string;

/**
 * One issue or pull request conversation: where a request came from and
 * where its replies go.
 */
export interface GithubThread {
  "number": number;
  "repository": GithubRepo;
}

/**
 * The watcher in `archon status`.
 * Tagged by `state`.
 */
export interface GithubWatchWatching {
  "enabled": boolean;
  "label_regex": string;
  "mention_regex": string;
  "operator_users"?: Array<string> | null;
  "orchestrator"?: AetherAgentId | null;
  "poll_interval_secs": number;
  "repositories": Array<WatchedRepo>;
  "revision": number;
  "state": "watching";
}

/**
 * archond could not ask Aether.
 */
export interface GithubWatchUnreachable {
  "error": string;
  "state": "unreachable";
}

export type GithubWatch = GithubWatchWatching | GithubWatchUnreachable;

/**
 * Archon's view of its aetherd's GitHub watcher.
 */
export interface GithubWatcherView {
  "enabled": boolean;
  "label_regex": string;
  "mention_regex": string;
  "operator_users"?: Array<string> | null;
  "orchestrator"?: AetherAgentId | null;
  "poll_interval_secs": number;
  "repositories": Array<WatchedRepo>;
  "revision": number;
}

export interface GraphInitiative {
  "atomic": boolean;
  "id": InitiativeId;
  "state": InitiativeState;
  "title": string;
}

/**
 * The range of change seqs still kept, for `ErrorCode::HistoryExpired`.
 */
export interface HistoryRange {
  "first_seq": number;
  "last_seq": number;
}

export interface HoldoutLift {
  "holdout_uses": number;
  "holdout_wins": number;
  "uses": number;
  "wins": number;
}

/**
 * Host context requested for newly created agents; full preserves host tools.
 */
export type HostContext = "full" | "lean";

/**
 * Why dispatch is held for the host: at least one of the two is set.
 */
export interface HostLoadHold {
  "load"?: Breach | null;
  "memory"?: Breach | null;
}

/**
 * The host-load hold as archond last judged it (ARCHITECTURE §7.3).
 */
export interface HostLoadStatus {
  "hold"?: HostLoadHold | null;
  "max_load_per_cpu"?: number | null;
  "min_available_memory_gb"?: number | null;
  "sample"?: HostSample | null;
  "unreadable"?: string | null;
}

/**
 * One reading of the host.
 */
export interface HostSample {
  "available_gb": number;
  "load_per_cpu": number;
}

/**
 * One image as a client sends it: Aether's `ImageAttachment`, field for field.
 */
export interface ImageAttachment {
  "data": string;
  "media_type": string;
  "name"?: string | null;
}

/**
 * An image a message carried, as the feed records it: no bytes.
 */
export interface ImageRef {
  "media_type": string;
  "name"?: string | null;
  "size_bytes": number;
}

/**
 * Its members are the items whose `initiative` names it.
 */
export interface Initiative {
  "atomic"?: boolean;
  "contract": string;
  "created_at": string;
  "id": InitiativeId;
  "state": InitiativeState;
  "title": string;
  "updated_at": string;
}

export interface InitiativeDetail {
  "initiative": Initiative;
  "members": Array<QueueItem>;
}

export type InitiativeId = string;

/**
 * One item an `enqueue_initiative` creates.
 */
export interface InitiativeMember {
  "after"?: Array<number>;
  "autonomy"?: AutonomyPatch;
  "brief": string;
  "model_plan"?: ModelPlanDraft | null;
  "onto"?: OntoTarget | null;
  "pipeline"?: string | null;
  "repo": RepoRef;
  "title"?: string | null;
}

export type InitiativeState = "shipping" | "done" | "failed" | "cancelled" | "open" | "succeeded";

/**
 * `stage` of catalog pipeline `from` (the item's own when left out),
 * placed after stage `after` (before the first unstarted one when left out).
 */
export interface InsertStage {
  "after"?: string | null;
  "from"?: string | null;
  "stage": string;
}

/**
 * What the latest look at an integration branch found.
 */
export type IntegrationHealth = "green" | "red" | "verifying" | "pending" | "unknown";

/**
 * One repository's integration branch.
 */
export interface IntegrationStatus {
  "branch": string;
  "culprit"?: string | null;
  "fix_item"?: QueueItemId | null;
  "head"?: string | null;
  "health": IntegrationHealth;
  "last_green"?: string | null;
  "merges_since_green": number;
  "problem"?: string | null;
  "repo": string;
  "verified_at"?: string | null;
}

export interface ItemDetail {
  "default_efforts"?: Record<string, Effort>;
  "delegations"?: Record<string, StageDelegation>;
  "gates"?: Array<OperatorGate>;
  "item": QueueItem;
  "merges"?: Array<MergeRequest>;
  "message_caps"?: Array<MessageCap>;
  "runs": Array<PipelineRun>;
  "spent_usd": number;
  "stage_warnings"?: Array<StageWarning>;
  "stages": Array<StageRun>;
  "steers"?: Array<SteerRecord>;
  "switched_to"?: string | null;
  "waiting_on"?: WaitingOn | null;
  "waiting_reason"?: WaitingReason | null;
}

export interface ItemPatch {
  "add_depends_on"?: Array<QueueItemId>;
  "autonomy"?: AutonomyPatch | null;
  "brief_append"?: string | null;
  "budget_usd"?: number | null;
  "model_plan"?: ModelPlanDraft | null;
  "model_reasons"?: Record<string, string> | null;
  "priority"?: Priority | null;
  "stage_timeouts"?: Record<string, number>;
  "tags"?: Array<string> | null;
  "title"?: string | null;
}

export interface ItemQuery {
  "before"?: string | null;
  "limit"?: number | null;
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "states"?: Array<string>;
  "tag"?: string | null;
  "text"?: string | null;
  "updated_since"?: string | null;
  "with_as_of"?: boolean;
}

/**
 * A queue item with what `archon queue list` shows about its runs. The
 * item's own fields are flattened in, so JSON readers of `Items` read it too.
 */
export interface ItemSummary {
  "autonomy"?: Autonomy;
  "brief": string;
  "budget_usd"?: number | null;
  "created_at": string;
  "dependencies": Array<QueueItemId>;
  "fresh_retries"?: number;
  "id": QueueItemId;
  "initiative"?: InitiativeId | null;
  "issue"?: GithubRef | null;
  "model_plan": ModelPlan;
  "model_reasons"?: Record<string, string> | null;
  "onto"?: OntoPullRequest | null;
  "origin"?: GithubThread | null;
  "pipeline": string;
  "pr"?: PrRef | null;
  "pr_url"?: string | null;
  "priority": Priority;
  "provider_switch"?: string | null;
  "repo"?: RepoId | null;
  "risk"?: RiskLevel | null;
  "routed_provider"?: string | null;
  "spent_usd": number;
  "stage"?: string | null;
  "stage_timeouts"?: Record<string, number>;
  "state": QueueState;
  "switched_to"?: string | null;
  "tags": Array<string>;
  "title": string;
  "updated_at": string;
  "waiting_on"?: WaitingOn | null;
  "waiting_reason"?: WaitingReason | null;
}

/**
 * One state transition of an item, as `item_history` lists them.
 */
export interface ItemTransition {
  "at": string;
  "by": TransitionActor;
  "from"?: QueueStateKind | null;
  "reason"?: string | null;
  "seq": number;
  "to": QueueStateKind;
}

/**
 * A model choice the evidence makes the default for one kind of stage.
 */
export interface LearnedRoute {
  "choice": ModelChoice;
  "passed": number;
  "pipeline": string;
  "repo"?: RepoId | null;
  "runs": number;
  "size"?: DiffSize | null;
  "stage": string;
}

/**
 * `archon learning why <item>`: every correction of the item's runs.
 */
export interface LearningWhy {
  "feedback": Array<FeedbackView>;
  "item": QueueItemId;
}

export interface Lesson {
  "cluster"?: LessonId | null;
  "confidence": number;
  "confirmed_at"?: string | null;
  "created_at": string;
  "id": LessonId;
  "item"?: QueueItemId | null;
  "kind": LessonKind;
  "last_used_at"?: string | null;
  "pinned"?: boolean;
  "refs"?: Array<LessonRef>;
  "scope": LessonScope;
  "source": LessonSource;
  "state": LessonState;
  "superseded_by"?: LessonId | null;
  "support"?: number;
  "text": string;
  "updated_at": string;
  "uses": number;
  "wins": number;
}

/**
 * What `record_lesson` and `archon lessons add` send.
 */
export interface LessonDraft {
  "item"?: QueueItemId | null;
  "kind": LessonKind;
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "text": string;
}

export type LessonId = string;

export type LessonKind = "practice" | "pitfall" | "routing" | "pipeline";

/**
 * An operator's edit; `state` accepts (`active`) or retracts a lesson.
 */
export interface LessonPatch {
  "kind"?: LessonKind | null;
  "pinned"?: boolean | null;
  "state"?: LessonState | null;
  "text"?: string | null;
}

export interface LessonQuery {
  "kind"?: LessonKind | null;
  "repo"?: RepoRef | null;
  "state"?: LessonState | null;
}

/**
 * One piece of structured provenance.
 */
export interface LessonRef {
  "kind": LessonRefKind;
  "value": string;
}

export type LessonRefKind = "item" | "run" | "pr";

/**
 * Where a lesson applies; all `None` is everywhere.
 */
export interface LessonScope {
  "pipeline"?: string | null;
  "repo"?: RepoId | null;
}

export type LessonSource = "operator" | "orchestrator" | "distilled";

export type LessonState = "active" | "retracted" | "proposed";

export interface LessonStats {
  "active": number;
  "corrected": number;
  "holdout_lift"?: HoldoutLift | null;
  "merged": number;
  "outcomes": number;
  "pipelines"?: Array<PipelineEvidence>;
  "proposed": number;
  "repo_names"?: Record<string, string>;
  "retracted": number;
  "routes": Array<LearnedRoute>;
}

/**
 * What shaped an item: the lessons its newest run was given, and why each
 * stage got its model where a learned default chose it.
 */
export interface LessonWhy {
  "item": QueueItemId;
  "lessons": Array<Lesson>;
  "routing": Record<string, string>;
  "run"?: PipelineRunId | null;
}

/**
 * Where a structural limit's value comes from.
 */
export type LimitSource = "default" | "config" | "fixed";

/**
 * How an item and a GitHub object relate.
 */
export type LinkRelation = "origin" | "imported" | "delivered" | "closes" | "mentioned";

/**
 * Who recorded a link.
 */
export type LinkSource = "watcher" | "operator" | "import" | "ship" | "backfill";

/**
 * What a `ship = local` item left on its branch, for review before
 * `archon merge` lands it.
 */
export interface LocalReview {
  "base": string;
  "blocker"?: string | null;
  "branch": string;
  "diffstat": string;
  "head": string;
  "item": QueueItemId;
  "repo_path": string;
}

/**
 * The relations an operator may add or remove with `link_issue`.
 */
export type ManualLinkRelation = "closes" | "mentioned";

/**
 * Aether's view of one MCP server (its `mcp_servers` reply).
 * Tagged by `state`.
 */
/**
 * Connected; agents started from now on get its `tools`.
 */
export interface McpHealthReady {
  "state": "ready";
  "tools": number;
}

/**
 * The last contact failed; Aether offers none of its tools.
 */
export interface McpHealthFailed {
  "error": string;
  "state": "failed";
}

/**
 * Configured, not contacted yet (it starts when an agent first uses it).
 */
export interface McpHealthUnknown {
  "state": "unknown";
}

/**
 * Aether has not loaded a server by that name.
 */
export interface McpHealthNotLoaded {
  "state": "not_loaded";
}

/**
 * archond could not ask Aether.
 */
export interface McpHealthUnreachable {
  "error": string;
  "state": "unreachable";
}

export type McpHealth =
  | McpHealthReady
  | McpHealthFailed
  | McpHealthUnknown
  | McpHealthNotLoaded
  | McpHealthUnreachable;

/**
 * `DaemonStatus::mcp`: the server's health now and the supervisor's
 * latest heal (ARCHITECTURE §16).
 */
export interface McpStatus {
  "failed_refreshes": number;
  "failed_relinks"?: number;
  "health": McpHealth;
  "last_heal"?: string | null;
}

/**
 * The memory's state when Archon first read it, before forgetting it.
 */
export type MemoryBefore = "current" | "forgotten" | "superseded";

/**
 * The forget a correction asked for, and Aether's answer.
 */
export interface MemoryCuration {
  "before"?: MemoryBefore | null;
  "error"?: string | null;
  "forgotten_at"?: string | null;
  "instance_id": string;
  "memory": AetherMemoryId;
  "restored_at"?: string | null;
  "state": CurationState;
  "updated_at": string;
}

export interface MemoryFeedback {
  "instance_id": string;
  "skipped"?: string | null;
  "tasks": Array<MemoryFeedbackTask>;
}

export interface MemoryFeedbackCounts {
  "partial_tasks": number;
  "pending": number;
  "retracted": number;
  "sent": number;
  "skipped": number;
}

export interface MemoryFeedbackEntry {
  "attempted": boolean;
  "dropped": boolean;
  "error"?: string | null;
  "failed"?: string | null;
  "feedback"?: AetherMemoryFeedbackId | null;
  "memory": AetherMemoryId;
  "retracted": boolean;
  "submission_horizon"?: string | null;
  "unconfirmed"?: boolean;
}

export interface MemoryFeedbackTask {
  "discovered": boolean;
  "error"?: string | null;
  "partial": boolean;
  "records": Array<MemoryFeedbackEntry>;
  "skipped"?: string | null;
  "task": AetherTaskId;
}

/**
 * An Aether memory as Archon shows it: where it came from, never its text.
 */
export interface MemoryView {
  "created_at": string;
  "forgotten_at"?: string | null;
  "id": AetherMemoryId;
  "kind": string;
  "origin": string;
  "pinned": boolean;
  "scope": string;
  "superseded_by"?: string | null;
}

/**
 * Who authorised a merge, as archond verified it.
 * Tagged by `via`.
 */
/**
 * `archon merge` on the operator's socket.
 */
export interface MergeAuthorityCli {
  "via": "cli";
}

/**
 * `/merge` in `archon chat`.
 */
export interface MergeAuthorityArchonChat {
  "via": "archon_chat";
}

/**
 * A whole-message `/merge` from the `user` actor in the orchestrator's
 * Aether chat, in dedicated mode.
 */
export interface MergeAuthorityAetherChat {
  "via": "aether_chat";
}

/**
 * A watcher-authenticated operator command, with its original thread.
 */
export interface MergeAuthorityGithub {
  "login": string;
  "number": number;
  "repository": string;
  "via": "github";
}

/**
 * The orchestrator's `request_merge`, which the operator confirmed by
 * answering the daemon's question `Q<number>`.
 */
export interface MergeAuthorityConfirmed {
  "number": number;
  "question": QuestionId;
  "via": "confirmed";
}

/**
 * The orchestrator's `request_merge` under the repository's
 * `grant_merge` setting, which only the operator sets.
 */
export interface MergeAuthorityGrant {
  "via": "grant";
}

export type MergeAuthority =
  | MergeAuthorityCli
  | MergeAuthorityArchonChat
  | MergeAuthorityAetherChat
  | MergeAuthorityGithub
  | MergeAuthorityConfirmed
  | MergeAuthorityGrant;

/**
 * Whether the pull request merges by itself once its checks allow.
 */
export type MergeMode = "manual" | "auto";

/**
 * One request to merge an item's pull request, and how it ended: the audit
 * record and the one source of truth for a waiting confirmation.
 */
export interface MergeRequest {
  "authority": MergeAuthority;
  "carried"?: CarriedReview | null;
  "checks"?: string | null;
  "head": string;
  "id": MergeRequestId;
  "ignore_checks": boolean;
  "item": QueueItemId;
  "mergeability_wait_since"?: string | null;
  "pr_url": string;
  "reason"?: string | null;
  "repo": RepoId;
  "requested_at": string;
  "run": PipelineRunId;
  "updated_at": string;
  "waited"?: boolean;
  "waived_by"?: string | null;
}

export type MergeRequestId = string;

/**
 * A stage agent whose `[limits] max_agent_messages` allowance is used up,
 * as told once to the orchestrator and shown by `get_item` and `queue_status`.
 */
export interface MessageCap {
  "agent": AetherAgentId;
  "at": string;
  "cap": number;
  "item": QueueItemId;
  "stage": string;
  "stage_run": StageRunId;
}

export type MessageKind = "relay" | "escalation" | "info";

/**
 * What the orchestrator chose for one agent: model and effort are
 * mandatory, mirroring Aether's `delegate`, which inherits neither.
 */
export interface ModelChoice {
  "effort": Effort;
  "model": ModelRef;
  "tier"?: TierChoice | null;
}

/**
 * A [`ModelChoice`] as a request gives it: an exact `model`, or a `tier`
 * (optionally on `provider`) that Archon resolves from Aether's catalog.
 * `effort` may be left out, and the stage's default effort stands in.
 */
export interface ModelChoiceDraft {
  "effort"?: Effort | null;
  "model"?: ModelRef | null;
  "provider"?: string | null;
  "tier"?: Tier | null;
}

export interface ModelEvidenceQuery {
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "since"?: DayBound | null;
  "stage"?: string | null;
}

export interface ModelEvidenceReport {
  "estimates_counted": boolean;
  "omitted": number;
  "pipelines"?: Array<PipelineEvidence>;
  "providers"?: Array<ProviderEvidenceRow>;
  "range": DayRange;
  "repo_names"?: Record<string, string>;
  "rows": Array<ModelEvidenceRow>;
  "tiers"?: Array<TierEvidenceRow>;
}

export interface ModelEvidenceRow {
  "attempts": number;
  "choice": ModelChoice;
  "completed": number;
  "host_context"?: HostContext;
  "median_cost_usd"?: number | null;
  "median_minutes"?: number | null;
  "p90_cost_usd"?: number | null;
  "passed": number;
  "stage": string;
  "verdicts": number;
}

/**
 * Per-stage choices for one pipeline run, keyed by stage id. A fan-out
 * stage lists one choice per parallel agent; every other stage lists one.
 */
export type ModelPlan = Record<string, Array<ModelChoice>>;

/**
 * A [`ModelPlan`] as a request gives it, keyed like it.
 */
export type ModelPlanDraft = Record<string, Array<ModelChoiceDraft>>;

export interface ModelPolicy {
  "allow"?: Array<ModelRef> | null;
  "deny": Array<ModelRef>;
  "max_effort"?: Effort | null;
}

/**
 * `provider/model`, e.g. `claude-code/opus`. Written as Aether's `{provider, model}` object; read from that object or from the string.
 */
export type ModelRef = string | Record<string, unknown>;

export interface ModelSegment {
  "choice": ModelChoice;
  "cost_before_usd": number;
  "delivery": string;
  "started_at": string;
}

export interface Notification {
  "at": string;
  "id": NotificationId;
  "item"?: QueueItemId | null;
  "kind": NotificationKind;
  "run"?: PipelineRunId | null;
  "text": string;
}

export type NotificationId = string;

/**
 * The orchestrator's `notify(kind=...)` vocabulary.
 */
export type NotificationKind = "question" | "decision" | "blocker" | "outcome" | "progress";

export interface Nudges {
  "independent_reviewers"?: boolean;
  "notes"?: string | null;
  "tier"?: Tier | null;
}

/**
 * An `onto` target's pull request as GitHub showed it at enqueue, frozen:
 * later work starts from `head_sha` and expects the head still there.
 */
export interface OntoPullRequest {
  "base_ref": string;
  "head_ref": string;
  "head_repository": string;
  "head_sha": string;
  "node_id"?: string | null;
  "pr": PullRequestRef;
  "review_only"?: boolean;
  "target": OntoTarget;
}

/**
 * The pull request an item delivers into instead of opening its own, as
 * enqueue's `onto` names it: another item's, a number, or an open branch's.
 */
export type OntoTarget = Record<string, unknown>;

/**
 * Tagged by `channel`.
 */
export interface OperatorChannelAetherChat {
  "channel": "aether_chat";
}

export interface OperatorChannelArchonChat {
  "channel": "archon_chat";
}

export interface OperatorChannelCli {
  "channel": "cli";
}

export interface OperatorChannelGithub {
  "channel": "github";
  "login": string;
  "number": number;
  "repository": GithubRepo;
}

export type OperatorChannel =
  | OperatorChannelAetherChat
  | OperatorChannelArchonChat
  | OperatorChannelCli
  | OperatorChannelGithub;

/**
 * One recorded correction, with mutable revocation, curation and delivery state.
 */
export interface OperatorFeedback {
  "attribution": Attribution2;
  "cause": CorrectionCause;
  "curation"?: MemoryCuration | null;
  "id": FeedbackId;
  "item": QueueItemId;
  "memory_feedback"?: MemoryFeedback | null;
  "observed_at": string;
  "reason": string;
  "request_key": string;
  "revoked_at"?: string | null;
  "run": PipelineRunId;
  "stage": string;
}

/**
 * One operator gate of a run, keyed by `(run, stage_id, attempt)`.
 */
export interface OperatorGate {
  "attempt": number;
  "decided_at"?: string | null;
  "decision"?: GateDecision | null;
  "item": QueueItemId;
  "location"?: string | null;
  "opened_at": string;
  "page"?: string | null;
  "question"?: QuestionId | null;
  "report"?: ReportId | null;
  "run": PipelineRunId;
  "stage_id": string;
  "subject": GateSubject;
  "summary": string;
  "withdrawn"?: string | null;
}

/**
 * A bounded copy of a message archond authenticated itself. The timestamp
 * is the event's original time, so reconnecting cannot refresh old consent.
 */
export interface OperatorMessage {
  "agent"?: AetherAgentId | null;
  "at": string;
  "channel": OperatorChannel;
  "seq": number;
  "text": string;
}

/**
 * What the operator set at one scope; unset fields come from the layer
 * beneath. An empty value is "nothing set".
 */
export interface OperatorSettings {
  "autonomy_default"?: AutonomyPatch;
  "autonomy_max"?: AutonomyPatch;
  "budget_usd_daily"?: number | null;
  "budget_usd_per_item"?: number | null;
  "budget_usd_total"?: number | null;
  "budgets_enabled"?: boolean | null;
  "checks"?: ChecksMode | null;
  "default_pipeline"?: string | null;
  "full_auto"?: boolean | null;
  "max_active_runs"?: number | null;
  "max_parallel_agents"?: number | null;
  "max_per_pipeline"?: Record<string, number> | null;
  "max_per_repo"?: number | null;
}

/**
 * The GitHub logins whose own requests carry the operator's authority.
 * A type of its own, so that no allowlist is ever saved as operator logins.
 */
export type OperatorUsers = Array<string>;

/**
 * How the running orchestrator generation compares with the one archond
 * would start now. A restart (`archon orchestrator restart`) catches it up.
 */
export interface OrchestratorFreshness {
  "created_by": string;
  "new_lessons": number;
  "prompt_outdated"?: boolean | null;
  "settings_drift"?: SettingsDrift | null;
  "tools_outdated": boolean;
}

export interface OrchestratorInfo {
  "agent": AetherAgentId;
  "effort": Effort;
  "freshness"?: OrchestratorFreshness | null;
  "generation"?: number;
  "model": ModelRef;
  "restarts": number;
  "status": string;
}

/**
 * A pairing just opened: the one-time code and where to use it.
 */
export interface PairingCode {
  "code": string;
  "expires_at": string;
  "urls": Array<string>;
}

/**
 * What is paused right now. Global pause stops all dispatch; the others
 * stop only their slice.
 */
export interface PauseState {
  "global": boolean;
  "pipelines": Array<string>;
  "repos": Array<RepoId>;
}

/**
 * Tagged by `scope`.
 */
export interface PauseTargetGlobal {
  "hard"?: boolean;
  "scope": "global";
}

export interface PauseTargetRepo {
  "name": RepoRef;
  "scope": "repo";
}

export interface PauseTargetPipeline {
  "name": string;
  "scope": "pipeline";
}

export type PauseTarget = PauseTargetGlobal | PauseTargetRepo | PauseTargetPipeline;

export interface PipelineEvidence {
  "correction_rate": number;
  "estimated_saving_usd"?: number | null;
  "median_cost_usd": number;
  "median_wall_secs": number;
  "pass_rate": number;
  "pipeline": string;
  "repo"?: RepoId | null;
  "runs": number;
  "scored"?: number;
}

/**
 * A pipeline directory that failed to load; its name is unavailable until
 * fixed, never silently replaced by a lower-precedence pipeline.
 */
export interface PipelineProblem {
  "error": string;
  "name": string;
  "path"?: string | null;
  "source": PipelineSource;
}

export interface PipelineRun {
  "aether_run"?: AetherRunId | null;
  "branch": string;
  "cost_usd": number;
  "finished_at"?: string | null;
  "id": PipelineRunId;
  "item": QueueItemId;
  "pipeline": string;
  "shipped"?: boolean;
  "started_at": string;
  "state": RunState;
  "team_root"?: string | null;
  "workspace": string;
}

export type PipelineRunId = string;

/**
 * Where a pipeline was found. Later sources replace earlier ones by name.
 */
export type PipelineSource = "builtin" | "user" | "repo";

export interface PipelineSummary {
  "description": string;
  "name": string;
  "nudges": Nudges;
  "path"?: string | null;
  "requires_repo": boolean;
  "source": PipelineSource;
  "stages": Array<StageSummary>;
  "workspace": WorkspaceMode;
}

/**
 * A pull request with what is known of its state; the reference's fields
 * sit beside the state.
 */
export interface PrRef {
  "checks"?: ChecksSummary | null;
  "host": string;
  "kind": GithubKind;
  "number": number;
  "repo": string;
  "review_decision"?: ReviewDecision | null;
  "state"?: PrState | null;
}

export type PrState = "open" | "draft" | "merged" | "closed";

export type Priority = "low" | "normal" | "high" | "critical";

/**
 * How fresh one status section is.
 */
export interface ProbeHealth {
  "as_of"?: string | null;
  "last_error"?: string | null;
  "stale": boolean;
}

/**
 * One program's versions as archond last saw them; `None` is unknown.
 */
export interface ProgramVersions {
  "installed"?: string | null;
  "latest"?: string | null;
  "running"?: string | null;
}

/**
 * How one promotion of an integration branch into its release branch ended.
 */
export interface Promotion {
  "branch": string;
  "head": string;
  "pull_request"?: string | null;
  "release": string;
  "repo": string;
  "risk"?: string | null;
}

/**
 * Whether a proposal is written and approved before implementing.
 */
export type ProposalMode = "lavish" | "off";

/**
 * One provider's attempts and completed cost: the split provider weights
 * achieved over the range.
 */
export interface ProviderEvidenceRow {
  "attempts": number;
  "completed": number;
  "cost_usd": number;
  "provider": string;
}

/**
 * One provider's quota as tier-only stage routing scores it.
 */
export interface ProviderQuotaStatus {
  "headroom"?: number | null;
  "next_reset"?: string | null;
  "provider": string;
  "reserve_percent": number;
  "windows": Array<QuotaWindow>;
}

/**
 * One pull request, as `https://<host>/<owner>/<name>/pull/<number>` names
 * it. Host and repository are lowercase, as GitHub matches them. Stored as
 * that URL, read back only when it is one.
 */
export type PullRequestRef = string;

/**
 * A question the orchestrator asked without blocking (Archon MCP
 * `ask_operator`). Aether's own `ask_user` questions are mirrored here too
 * so one place holds everything awaiting an answer.
 */
export interface Question {
  "aether"?: AetherQuestionRef | null;
  "answer"?: string | null;
  "answered_at"?: string | null;
  "asked_at": string;
  "asked_by"?: AetherAgentId | null;
  "id": QuestionId;
  "item"?: QueueItemId | null;
  "number"?: number;
  "options": Array<string>;
  "text": string;
}

export type QuestionId = string;

export interface QueueItem {
  "autonomy"?: Autonomy;
  "brief": string;
  "budget_usd"?: number | null;
  "created_at": string;
  "dependencies": Array<QueueItemId>;
  "fresh_retries"?: number;
  "id": QueueItemId;
  "initiative"?: InitiativeId | null;
  "model_plan": ModelPlan;
  "model_reasons"?: Record<string, string> | null;
  "onto"?: OntoPullRequest | null;
  "origin"?: GithubThread | null;
  "pipeline": string;
  "priority": Priority;
  "provider_switch"?: string | null;
  "repo"?: RepoId | null;
  "risk"?: RiskLevel | null;
  "routed_provider"?: string | null;
  "stage_timeouts"?: Record<string, number>;
  "state": QueueState;
  "tags": Array<string>;
  "title": string;
  "updated_at": string;
}

export type QueueItemId = string;

/**
 * Lifecycle of a backlog item. Terminal states never leave their column
 * except through an explicit operator `retry`, which re-queues.
 * Tagged by `state`.
 */
/**
 * Waiting for dependencies, capacity, or an unpause.
 */
export interface QueueStateQueued {
  "state": "queued";
}

/**
 * Chosen by the dispatcher; worktree and first stage being prepared.
 */
export interface QueueStateDispatching {
  "state": "dispatching";
}

/**
 * A pipeline run is active.
 */
export interface QueueStateRunning {
  "run": PipelineRunId;
  "state": "running";
}

/**
 * A dependency failed or was cancelled; needs an operator decision.
 */
export interface QueueStateBlocked {
  "reason": string;
  "state": "blocked";
}

export interface QueueStateSucceeded {
  "run": PipelineRunId;
  "state": "succeeded";
  "summary": string;
}

export interface QueueStateFailed {
  "error": string;
  "run"?: PipelineRunId | null;
  "state": "failed";
}

export interface QueueStateCancelled {
  "reason"?: string | null;
  "state": "cancelled";
}

export type QueueState =
  | QueueStateQueued
  | QueueStateDispatching
  | QueueStateRunning
  | QueueStateBlocked
  | QueueStateSucceeded
  | QueueStateFailed
  | QueueStateCancelled;

/**
 * A queue item's state without its data: the names `QueueState::kind`
 * gives and `list_items` filters accept.
 */
export type QueueStateKind = "queued" | "dispatching" | "running" | "blocked" | "succeeded" | "failed" | "cancelled";

/**
 * `DaemonStatus::quota`.
 */
export interface QuotaStatus {
  "as_of": string;
  "providers": Array<ProviderQuotaStatus>;
}

/**
 * One usage window as aetherd reports it (its protocol's `WindowHeadroom`).
 */
export interface QuotaWindow {
  "name": string;
  "remaining_percent": number;
  "resets_at"?: string | null;
  "resets_in_seconds"?: number | null;
  "window_minutes"?: number | null;
}

/**
 * The build caches deleted from one worktree.
 */
export interface Reclaimed {
  "bytes": number;
  "path": string;
}

/**
 * `DaemonStatus::recovery`: how far archond is in recovering its runs.
 * Until it is done, changes are refused and nothing is dispatched.
 */
export interface RecoveryProgress {
  "runs_done": number;
  "runs_total"?: number | null;
}

/**
 * How the release hook ran after the merge.
 */
export interface ReleaseHook {
  "command": string;
  "excerpt": string;
  "passed": boolean;
}

/**
 * A change to the runtime settings; `None` keeps a field.
 */
export interface RemoteChange {
  "add_origins"?: Array<string>;
  "enabled"?: boolean | null;
  "remove_origins"?: Array<string>;
}

/**
 * What `remote_status` and every remote change answer.
 */
export interface RemoteView {
  "address": string;
  "devices": Array<DeviceView>;
  "enabled": boolean;
  "error"?: string | null;
  "listening"?: string | null;
  "mounted_at"?: string | null;
  "pairing_expires_at"?: string | null;
  "public_origins": Array<string>;
  "trusted_origins"?: Array<string>;
}

/**
 * Adjacently tagged like Aether's replies: `{"reply":"item","data":{...}}`.
 * Tagged by `reply`.
 */
export interface ReplyGuide {
  "data": Record<string, unknown>;
  "reply": "guide";
}

export interface ReplyMessageReceipt {
  "data": Record<string, unknown>;
  "reply": "message_receipt";
}

export interface ReplyWelcome {
  "data": Welcome;
  "reply": "welcome";
}

export interface ReplyOk {
  "reply": "ok";
}

export interface ReplyStatus {
  "data": DaemonStatus;
  "reply": "status";
}

export interface ReplyRepo {
  "data": Repo;
  "reply": "repo";
}

export interface ReplyRepos {
  "data": Array<Repo>;
  "reply": "repos";
}

export interface ReplyItem {
  "data": QueueItem;
  "reply": "item";
}

/**
 * `Item`, for a change that set a dollar budget while budgets are off.
 */
export interface ReplyItemBudgetsOff {
  "data": QueueItem;
  "reply": "item_budgets_off";
}

export interface ReplyItems {
  "data": Array<QueueItem>;
  "reply": "items";
}

export interface ReplyItemSummaries {
  "data": Array<ItemSummary>;
  "reply": "item_summaries";
}

/**
 * `list_item_summaries` asked `with_as_of`: the summaries and the last
 * change seq committed in their read transaction.
 */
export interface ReplyItemSummariesAt {
  "data": Record<string, unknown>;
  "reply": "item_summaries_at";
}

export interface ReplyItemDetail {
  "data": ItemDetail;
  "reply": "item_detail";
}

export interface ReplyStageDetail {
  "data": StageDetail;
  "reply": "stage_detail";
}

export interface ReplyCatalog {
  "data": CatalogListing;
  "reply": "catalog";
}

export interface ReplyModelPolicy {
  "data": ModelPolicy;
  "reply": "model_policy";
}

export interface ReplyQuestion {
  "data": Question;
  "reply": "question";
}

export interface ReplyQuestions {
  "data": Array<Question>;
  "reply": "questions";
}

export interface ReplySubscribed {
  "data": Record<string, unknown>;
  "reply": "subscribed";
}

export interface ReplyFeed {
  "data": Array<FeedItem>;
  "reply": "feed";
}

export interface ReplyReport {
  "data": ReportPublished;
  "reply": "report";
}

export interface ReplyReports {
  "data": Array<ReportSummary>;
  "reply": "reports";
}

export interface ReplyOrchestrator {
  "data": OrchestratorInfo;
  "reply": "orchestrator";
}

export interface ReplyAutonomy {
  "data": AutonomyView;
  "reply": "autonomy";
}

export interface ReplyDirective {
  "data": Directive;
  "reply": "directive";
}

/**
 * How many directives a clear removed.
 */
export interface ReplyCleared {
  "data": number;
  "reply": "cleared";
}

export interface ReplyGithubWatcher {
  "data": GithubWatcherView;
  "reply": "github_watcher";
}

/**
 * The comment `github_reply` posted.
 */
export interface ReplyGithubComment {
  "data": Record<string, unknown>;
  "reply": "github_comment";
}

/**
 * An upgrade was accepted; what it will do.
 */
export interface ReplyUpgradeStarted {
  "data": Record<string, unknown>;
  "reply": "upgrade_started";
}

export interface ReplyVersions {
  "data": Versions;
  "reply": "versions";
}

export interface ReplyGc {
  "data": GcReport;
  "reply": "gc";
}

export interface ReplyArtifacts {
  "data": ArtifactListing;
  "reply": "artifacts";
}

export interface ReplyArtifactPage {
  "data": ArtifactPage;
  "reply": "artifact_page";
}

export interface ReplyCostReport {
  "data": CostReport;
  "reply": "cost_report";
}

export interface ReplyModelEvidence {
  "data": ModelEvidenceReport;
  "reply": "model_evidence";
}

export interface ReplyInitiative {
  "data": InitiativeDetail;
  "reply": "initiative";
}

export interface ReplyInitiatives {
  "data": Array<Initiative>;
  "reply": "initiatives";
}

export interface ReplyLesson {
  "data": Lesson;
  "reply": "lesson";
}

export interface ReplyLessons {
  "data": Array<Lesson>;
  "reply": "lessons";
}

export interface ReplyLessonsWhy {
  "data": LessonWhy;
  "reply": "lessons_why";
}

export interface ReplyLessonStats {
  "data": LessonStats;
  "reply": "lesson_stats";
}

export interface ReplyFeedback {
  "data": FeedbackReceipt;
  "reply": "feedback";
}

export interface ReplyLearningWhy {
  "data": LearningWhy;
  "reply": "learning_why";
}

export interface ReplyMerge {
  "data": MergeRequest;
  "reply": "merge";
}

export interface ReplyLocalReview {
  "data": LocalReview;
  "reply": "local_review";
}

export interface ReplyPromotion {
  "data": Promotion;
  "reply": "promotion";
}

export interface ReplySettings {
  "data": SettingsView;
  "reply": "settings";
}

export interface ReplyRequestStatus {
  "data": RequestStatus;
  "reply": "request_status";
}

export interface ReplyRemote {
  "data": RemoteView;
  "reply": "remote";
}

export interface ReplyRemotePairing {
  "data": PairingCode;
  "reply": "remote_pairing";
}

export interface ReplyChanges {
  "data": Record<string, unknown>;
  "reply": "changes";
}

export interface ReplyItemHistory {
  "data": Record<string, unknown>;
  "reply": "item_history";
}

export interface ReplyTaskGraph {
  "data": TaskGraph;
  "reply": "task_graph";
}

export interface ReplyRunPlan {
  "data": RunPlan;
  "reply": "run_plan";
}

export interface ReplyItemLinks {
  "data": Record<string, unknown>;
  "reply": "item_links";
}

export interface ReplyIssueItems {
  "data": Record<string, unknown>;
  "reply": "issue_items";
}

export interface ReplyGithubObjects {
  "data": Record<string, unknown>;
  "reply": "github_objects";
}

export type Reply =
  | ReplyGuide
  | ReplyMessageReceipt
  | ReplyWelcome
  | ReplyOk
  | ReplyStatus
  | ReplyRepo
  | ReplyRepos
  | ReplyItem
  | ReplyItemBudgetsOff
  | ReplyItems
  | ReplyItemSummaries
  | ReplyItemSummariesAt
  | ReplyItemDetail
  | ReplyStageDetail
  | ReplyCatalog
  | ReplyModelPolicy
  | ReplyQuestion
  | ReplyQuestions
  | ReplySubscribed
  | ReplyFeed
  | ReplyReport
  | ReplyReports
  | ReplyOrchestrator
  | ReplyAutonomy
  | ReplyDirective
  | ReplyCleared
  | ReplyGithubWatcher
  | ReplyGithubComment
  | ReplyUpgradeStarted
  | ReplyVersions
  | ReplyGc
  | ReplyArtifacts
  | ReplyArtifactPage
  | ReplyCostReport
  | ReplyModelEvidence
  | ReplyInitiative
  | ReplyInitiatives
  | ReplyLesson
  | ReplyLessons
  | ReplyLessonsWhy
  | ReplyLessonStats
  | ReplyFeedback
  | ReplyLearningWhy
  | ReplyMerge
  | ReplyLocalReview
  | ReplyPromotion
  | ReplySettings
  | ReplyRequestStatus
  | ReplyRemote
  | ReplyRemotePairing
  | ReplyChanges
  | ReplyItemHistory
  | ReplyTaskGraph
  | ReplyRunPlan
  | ReplyItemLinks
  | ReplyIssueItems
  | ReplyGithubObjects;

export interface Repo {
  "added_at": string;
  "attribution"?: Attribution;
  "autonomy_max"?: AutonomyPatch;
  "base_branch"?: string | null;
  "confirmed_fast_check"?: string | null;
  "confirmed_full_check"?: string | null;
  "confirmed_release_check"?: string | null;
  "default_pipeline"?: string | null;
  "gh_user"?: GhUser | null;
  "grant_merge"?: boolean;
  "id": RepoId;
  "merge_waive_checks"?: Array<string>;
  "name": string;
  "path": string;
  "ship": ShipMode;
  "tiers"?: RepoTiers;
}

export interface RepoAdd {
  "auto_merge"?: boolean;
  "base_branch"?: string | null;
  "default_pipeline"?: string | null;
  "github_watch"?: boolean;
  "name"?: string | null;
  "path": string;
  "ship"?: ShipMode | null;
}

export type RepoId = string;

export interface RepoPatch {
  "attribution"?: Attribution | null;
  "autonomy_max"?: AutonomyPatch | null;
  "base_branch"?: Clearable;
  "default_pipeline"?: Clearable;
  "gh_user"?: Clearable2;
  "grant_merge"?: boolean | null;
  "merge_waive_checks"?: Array<string> | null;
  "ship"?: ShipMode | null;
  "tiers"?: TiersPatch;
}

/**
 * A repository by registry name or `repo_…` id.
 */
export type RepoRef = string;

/**
 * The shape of `.archon/repo.toml` and of the registry's `tiers`; the
 * registry's values win field by field ([`RepoTiers::over`]).
 */
export interface RepoTiers {
  "checks"?: Checks;
  "integration_branch"?: string | null;
  "release_branch"?: string | null;
}

export type ReportId = string;

/**
 * Whether a change report is written before shipping, and whether the
 * operator must approve it first.
 */
export type ReportMode = "before-pr" | "with-pr" | "off";

export interface ReportPublished {
  "location": string;
  "published": boolean;
  "report": ReportId;
  "run"?: PipelineRunId | null;
}

/**
 * Where a recorded report stands (ARCHITECTURE §14).
 */
export type ReportState = "ended" | "unpublished" | "open" | "disconnected" | "stopped";

/**
 * One recorded report, as `ListReports` shows it.
 */
export interface ReportSummary {
  "item"?: QueueItemId | null;
  "location"?: string | null;
  "path": string;
  "reopen_question"?: QuestionId | null;
  "report": ReportId;
  "state": ReportState;
  "title": string;
  "updated_at": string;
}

/**
 * Everything a client can ask `archond`. MCP tools map 1:1 onto a subset
 * (ARCHITECTURE §11).
 * Tagged by `cmd`.
 */
/**
 * A tool call `archon mcp` forwards for the Aether agent that made it:
 * only the orchestrator of now may change Archon (ARCHITECTURE §11).
 */
export interface RequestForAgent {
  "caller": AetherAgentId;
  "cmd": "for_agent";
  "request": Request;
}

/**
 * A change `archon mcp` forwards under its own request id, recorded with
 * its outcome for `request_status` (`archon_core::tracked`).
 */
export interface RequestTracked {
  "cmd": "tracked";
  "idempotency_key"?: string | null;
  "request": Request;
  "request_id": RequestId;
}

/**
 * What became of a tracked change the caller made.
 */
export interface RequestRequestStatus {
  "cmd": "request_status";
  "request": RequestId;
}

export interface RequestMessageOrchestrator {
  "caller"?: AetherAgentId | null;
  "cmd": "message_orchestrator";
  "kind"?: MessageKind;
  "text": string;
}

/**
 * A stage agent keeps files from its workspace in its item's artifact
 * store (ARCHITECTURE §13.5).
 */
export interface RequestPublishArtifact {
  "caller"?: AetherAgentId | null;
  "cmd": "publish_artifact";
  "name"?: string | null;
  "path": string;
}

export interface RequestListArtifacts {
  "cmd": "list_artifacts";
  "item": QueueItemId;
}

export interface RequestReadArtifact {
  "cmd": "read_artifact";
  "item": QueueItemId;
  "limit"?: number | null;
  "name": string;
  "offset"?: number;
}

export interface RequestHello {
  "client": ClientKind;
  "client_version": string;
  "cmd": "hello";
  "features"?: Array<string>;
  "protocol_version": number;
}

export interface RequestStatusVariant {
  "cmd": "status";
  "waiting_limit"?: number | null;
  "waiting_offset"?: number;
}

export interface RequestGuide {
  "cmd": "guide";
  "topic": string;
}

/**
 * CLI only.
 */
export interface RequestShutdown {
  "cmd": "shutdown";
}

export interface RequestRepoAdd {
  "auto_merge"?: boolean;
  "base_branch"?: string | null;
  "cmd": "repo_add";
  "default_pipeline"?: string | null;
  "github_watch"?: boolean;
  "name"?: string | null;
  "path": string;
  "ship"?: ShipMode | null;
}

export interface RequestRepoList {
  "cmd": "repo_list";
}

/**
 * CLI only.
 */
export interface RequestRepoRemove {
  "cmd": "repo_remove";
  "repo": RepoRef;
}

/**
 * CLI only; the one path to a repository's autonomy ceiling.
 */
export interface RequestRepoUpdate {
  "cmd": "repo_update";
  "patch": RepoPatch;
  "repo": RepoRef;
}

export interface RequestEnqueue {
  "autonomy"?: AutonomyPatch;
  "brief": string;
  "budget_usd"?: number | null;
  "cmd": "enqueue";
  "depends_on"?: Array<QueueItemId>;
  "idempotency_key"?: string | null;
  "model_plan"?: ModelPlanDraft;
  "model_reasons"?: Record<string, string> | null;
  "onto"?: OntoTarget | null;
  "origin"?: GithubThread | null;
  "pipeline"?: string | null;
  "priority"?: Priority;
  "repo"?: RepoRef | null;
  "stage_timeouts"?: Record<string, number>;
  "tags"?: Array<string>;
  "title": string;
}

export interface RequestListItems {
  "before"?: string | null;
  "cmd": "list_items";
  "limit"?: number | null;
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "states"?: Array<string>;
  "tag"?: string | null;
  "text"?: string | null;
  "updated_since"?: string | null;
  "with_as_of"?: boolean;
}

/**
 * Like `ListItems`, each item with its live stage, spend and PR URL.
 */
export interface RequestListItemSummaries {
  "before"?: string | null;
  "cmd": "list_item_summaries";
  "limit"?: number | null;
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "states"?: Array<string>;
  "tag"?: string | null;
  "text"?: string | null;
  "updated_since"?: string | null;
  "with_as_of"?: boolean;
}

export interface RequestGetItem {
  "cmd": "get_item";
  "item": QueueItemId;
}

/**
 * The stage runs of an item's latest run, or one stage's full output.
 */
export interface RequestGetStage {
  "attempt"?: number | null;
  "cmd": "get_stage";
  "item": QueueItemId;
  "limit"?: number | null;
  "offset"?: number;
  "run"?: PipelineRunId | null;
  "slot"?: number | null;
  "stage"?: string | null;
}

/**
 * The one item whose id starts with `prefix` (see
 * [`crate::id::queue_item_prefix`]). An unknown prefix is `NotFound`;
 * an ambiguous one is refused, naming the items it matches.
 */
export interface RequestResolveItem {
  "cmd": "resolve_item";
  "prefix": string;
}

export interface RequestUpdateItem {
  "cmd": "update_item";
  "item": QueueItemId;
  "patch": ItemPatch;
}

export interface RequestCancelItem {
  "cmd": "cancel_item";
  "item": QueueItemId;
  "reason"?: string | null;
}

export interface RequestRetryItem {
  "brief_append"?: string | null;
  "cmd": "retry_item";
  "fresh"?: boolean;
  "item": QueueItemId;
  "model_plan"?: ModelPlanDraft | null;
  "model_reasons"?: Record<string, string> | null;
  "stage_timeouts"?: Record<string, number>;
}

/**
 * Merge the finished item's base branch, fetched afresh, into its branch:
 * a new item on the same branch and pull request (ARCHITECTURE §13).
 * Its plan and stage timeouts are the stage defaults unless given here,
 * never the finished item's own.
 */
export interface RequestRefreshItem {
  "cmd": "refresh_item";
  "item": QueueItemId;
  "model_plan"?: ModelPlanDraft | null;
  "model_reasons"?: Record<string, string> | null;
  "stage_timeouts"?: Record<string, number>;
}

export interface RequestPause {
  "cmd": "pause";
}

export interface RequestResume {
  "cmd": "resume";
}

/**
 * Operator only; persisted in the global settings overlay.
 */
export interface RequestSetCaps {
  "caps": Caps;
  "cmd": "set_caps";
}

/**
 * The operator settings stored for a repository, or globally, and the
 * values in force with their sources.
 */
export interface RequestSettings {
  "cmd": "settings";
  "repo"?: RepoRef | null;
}

/**
 * Operator only: replace the overlay stored at one scope whole.
 */
export interface RequestSetSettings {
  "cmd": "set_settings";
  "repo"?: RepoRef | null;
  "settings": OperatorSettings;
}

/**
 * Operator only: forget the overlay stored at one scope.
 */
export interface RequestResetSettings {
  "cmd": "reset_settings";
  "repo"?: RepoRef | null;
}

export interface RequestListPipelines {
  "cmd": "list_pipelines";
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
}

export interface RequestModelPolicy {
  "cmd": "model_policy";
  "repo"?: RepoRef | null;
}

/**
 * Operator chat line to the orchestrator. CLI only. `text` may be
 * blank when there is at least one image.
 */
export interface RequestSay {
  "cmd": "say";
  "images"?: Array<ImageAttachment>;
  "text": string;
}

export interface RequestNotify {
  "cmd": "notify";
  "item"?: QueueItemId | null;
  "kind": NotificationKind;
  "text": string;
}

/**
 * Resolve the notifications archond posted to Aether's notification
 * center: the open blockers of one item, or every open blocker and
 * outcome when `item` is `None`.
 */
export interface RequestClearNotifications {
  "cmd": "clear_notifications";
  "item"?: QueueItemId | null;
}

export interface RequestAskOperator {
  "caller"?: AetherAgentId | null;
  "cmd": "ask_operator";
  "item"?: QueueItemId | null;
  "options"?: Array<string>;
  "text": string;
}

/**
 * CLI only: the operator's answer, never the orchestrator's.
 */
export interface RequestAnswer {
  "chat"?: boolean;
  "cmd": "answer";
  "question": QuestionId;
  "text": string;
}

/**
 * The orchestrator records that the operator answered, in chat, a
 * question it asked with `ask_operator`. Never a gate's question.
 */
export interface RequestRecordChatAnswer {
  "answer": string;
  "caller"?: AetherAgentId | null;
  "cmd": "record_chat_answer";
  "question": QuestionId;
}

export interface RequestListQuestions {
  "cmd": "list_questions";
  "open_only": boolean;
}

/**
 * Stream feed items with `seq > after` (`None`: live only).
 */
export interface RequestSubscribeFeed {
  "after"?: number | null;
  "cmd": "subscribe_feed";
  "verbose": boolean;
}

export interface RequestFeedHistory {
  "before"?: number | null;
  "cmd": "feed_history";
  "limit": number;
  "verbose": boolean;
}

export interface RequestPublishReport {
  "cmd": "publish_report";
  "item"?: QueueItemId | null;
  "path": string;
  "title": string;
}

/**
 * Every recorded report, most recently changed first.
 */
export interface RequestListReports {
  "cmd": "list_reports";
}

/**
 * Show a recorded report again at the operator's request: resume its
 * Lavish session, even one they ended, or name its path without
 * Lavish. A failed reopen leaves the report as it was.
 */
export interface RequestReopenReport {
  "cmd": "reopen_report";
  "report": ReportId;
}

export interface RequestOrchestratorStatus {
  "cmd": "orchestrator_status";
}

/**
 * CLI only: ensure the orchestrator is live, without sending a message.
 */
export interface RequestOrchestratorAttach {
  "cmd": "orchestrator_attach";
}

/**
 * CLI only: cancel and recreate the orchestrator agent.
 */
export interface RequestOrchestratorRestart {
  "cmd": "orchestrator_restart";
}

/**
 * Operator only: install, drain, then restart archond and Archon's own
 * aetherd. Accepted at once; each step is reported in the feed.
 */
export interface RequestUpgrade {
  "aether"?: UpgradeTarget | null;
  "archon"?: UpgradeTarget | null;
  "cmd": "upgrade";
  "drain_secs": number;
  "keep_aether"?: boolean;
  "requeue"?: boolean;
}

/**
 * Operator only, as it asks the network: the latest releases of archon
 * and aether, kept for `status` and `queue_status`.
 */
export interface RequestCheckUpdates {
  "cmd": "check_updates";
}

/**
 * CLI only: collect what finished runs left (ARCHITECTURE §13.2).
 * Worktrees go once their runs finished `older_than_secs` ago
 * (`None`: `worktrees.gc_after`); with `force`, even with changes.
 */
export interface RequestGc {
  "cmd": "gc";
  "force": boolean;
  "older_than_secs"?: number | null;
  "preview"?: boolean;
}

/**
 * Spend over a range of local days, grouped by day, repository,
 * pipeline, model, item or stage (ARCHITECTURE §5.3).
 */
export interface RequestCostReport {
  "by"?: CostGroup;
  "cmd": "cost_report";
  "since"?: DayBound | null;
  "until"?: DayBound | null;
}

export interface RequestModelEvidence {
  "cmd": "model_evidence";
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "since"?: DayBound | null;
  "stage"?: string | null;
}

/**
 * Autonomy defaults, ceiling and directives, for one repository or
 * globally (ARCHITECTURE §18).
 */
export interface RequestAutonomy {
  "cmd": "autonomy";
  "repo"?: RepoRef | null;
}

/**
 * Record a standing operator directive. It steers the orchestrator's
 * choices and never raises the ceiling, so the MCP proxy may send it.
 */
export interface RequestSetDirective {
  "cmd": "set_directive";
  "expires_in_hours"?: number | null;
  "text": string;
}

/**
 * Clear one directive, or every directive when `None`.
 */
export interface RequestClearDirective {
  "cmd": "clear_directive";
  "directive"?: DirectiveId | null;
}

/**
 * Archon's GitHub watcher in its aetherd.
 */
export interface RequestGithubWatcher {
  "cmd": "github_watcher";
}

/**
 * CLI only: GitHub content must never change what is watched.
 */
export interface RequestConfigureGithubWatcher {
  "change": WatcherChange;
  "cmd": "configure_github_watcher";
}

/**
 * Comment on a thread of a repository Archon watches; archond appends
 * the reply marker so the comment never re-triggers the watcher.
 */
export interface RequestGithubReply {
  "body": string;
  "cmd": "github_reply";
  "thread": GithubThread;
}

/**
 * Several items, usually in several repositories, built against one
 * shared contract.
 */
export interface RequestEnqueueInitiative {
  "atomic"?: boolean;
  "cmd": "enqueue_initiative";
  "contract": string;
  "idempotency_key"?: string | null;
  "members": Array<InitiativeMember>;
  "title": string;
}

export interface RequestListInitiatives {
  "cmd": "list_initiatives";
}

export interface RequestCancelInitiative {
  "cmd": "cancel_initiative";
  "initiative": InitiativeId;
}

export interface RequestRetryInitiative {
  "cmd": "retry_initiative";
  "initiative": InitiativeId;
}

export interface RequestGetInitiative {
  "cmd": "get_initiative";
  "initiative": InitiativeId;
}

/**
 * Append to the shared contract or replace it; live members are told.
 */
export interface RequestUpdateInitiative {
  "cmd": "update_initiative";
  "initiative": InitiativeId;
}

/**
 * Change a queued or running item without cancelling it.
 */
export interface RequestSteerItem {
  "cmd": "steer_item";
  "item": QueueItemId;
  "steer": Steer;
}

export interface RequestRecordLesson {
  "cmd": "record_lesson";
  "item"?: QueueItemId | null;
  "kind": LessonKind;
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "text": string;
}

export interface RequestLessons {
  "cmd": "lessons";
  "kind"?: LessonKind | null;
  "repo"?: RepoRef | null;
  "state"?: LessonState | null;
}

/**
 * CLI only: the operator edits, pins, accepts or retracts a lesson,
 * and that always wins over what was learned.
 */
export interface RequestUpdateLesson {
  "cmd": "update_lesson";
  "lesson": LessonId;
  "patch": LessonPatch;
}

/**
 * The lessons and learned defaults that shaped an item.
 */
export interface RequestLessonsWhy {
  "cmd": "lessons_why";
  "item": QueueItemId;
}

export interface RequestLessonStats {
  "cmd": "lesson_stats";
  "repo"?: RepoRef | null;
}

/**
 * CLI only: the operator corrects one stage of a finished run.
 */
export interface RequestLearningCorrect {
  "cause": CorrectionCause;
  "cmd": "learning_correct";
  "forget_memory"?: AetherMemoryId | null;
  "item": QueueItemId;
  "reason": string;
  "run": PipelineRunId;
  "stage": string;
}

/**
 * The corrections of an item's runs and what each does now.
 */
export interface RequestLearningWhy {
  "cmd": "learning_why";
  "item": QueueItemId;
}

/**
 * CLI only: revoke a correction's model penalty; its memory stays as is.
 */
export interface RequestLearningUndo {
  "cmd": "learning_undo";
  "feedback": FeedbackId;
}

/**
 * CLI only: restore the memory a correction forgot.
 */
export interface RequestLearningRestoreMemory {
  "cmd": "learning_restore_memory";
  "feedback": FeedbackId;
}

/**
 * CLI only: retry a correction's pending or failed memory forget.
 */
export interface RequestLearningRetryMemory {
  "cmd": "learning_retry_memory";
  "feedback": FeedbackId;
}

/**
 * CLI only: the operator merges an item's delivered pull request now,
 * once archond's checks pass (ARCHITECTURE §18.6).
 */
export interface RequestMergeItem {
  "chat"?: boolean;
  "cmd": "merge_item";
  "ignore_checks"?: boolean;
  "item": QueueItemId;
}

/**
 * What a `ship = local` item's branch changes against its base, and
 * whether `MergeItem` would land it now.
 */
export interface RequestItemDiff {
  "cmd": "item_diff";
  "full"?: boolean;
  "item": QueueItemId;
}

/**
 * Whole-message command on the operator socket; never exposed over MCP.
 */
export interface RequestOperatorCommand {
  "cmd": "operator_command";
  "text": string;
}

export interface RequestUndoAuthority {
  "change": number;
  "cmd": "undo_authority";
}

/**
 * The orchestrator cites a message observed on archond's own stream.
 */
export interface RequestRequestAuthority {
  "action": Request;
  "caller"?: AetherAgentId | null;
  "cmd": "request_authority";
  "quote": string;
  "reason": string;
  "utterance": number;
}

/**
 * Promote the repository's integration branch into its release branch
 * once its head is green; an agent's call merges only under `grant_merge`.
 */
export interface RequestPromote {
  "cmd": "promote";
  "repo": RepoRef;
}

/**
 * Request a merge; archond confirms unless `grant_merge` permits it.
 */
export interface RequestRequestMerge {
  "cmd": "request_merge";
  "item": QueueItemId;
  "reason"?: string | null;
}

/**
 * Remote access (docs/remote.md): the listener, paired devices and any
 * open pairing. Operator only, and never served to the web app itself.
 */
export interface RequestRemoteStatus {
  "cmd": "remote_status";
}

export interface RequestConfigureRemote {
  "change": RemoteChange;
  "cmd": "configure_remote";
}

/**
 * Open a pairing with a new one-time code, replacing any open one.
 */
export interface RequestRemotePair {
  "cmd": "remote_pair";
  "role"?: DeviceRole | null;
}

/**
 * Unpair a device by id or name, closing its sessions at once.
 */
export interface RequestRemoteRevoke {
  "cmd": "remote_revoke";
  "device": string;
}

/**
 * Stream changes with `seq > after` (`None`: live only); feature `changes`.
 */
export interface RequestSubscribeChanges {
  "after"?: number | null;
  "cmd": "subscribe_changes";
}

/**
 * The changes with `seq > after`, oldest first; feature `changes`.
 */
export interface RequestChangesPage {
  "after": number;
  "cmd": "changes_page";
  "limit"?: number | null;
}

/**
 * An item's state transitions, oldest first; feature `item_history`.
 */
export interface RequestItemHistory {
  "cmd": "item_history";
  "item": QueueItemId;
  "limit"?: number | null;
}

/**
 * Items and their typed edges as of one change; feature `task_graph`.
 */
export interface RequestTaskGraph {
  "cmd": "task_graph";
  "filter"?: TaskFilter;
  "limit"?: number | null;
}

/**
 * A run's stages as it executed them; feature `run_plan`.
 */
export interface RequestRunPlan {
  "cmd": "run_plan";
  "run": PipelineRunId;
}

/**
 * An item's GitHub links and their cached objects; feature `github_links`.
 */
export interface RequestItemLinks {
  "cmd": "item_links";
  "item": QueueItemId;
}

/**
 * The items linked to one issue or pull request; feature `github_links`.
 */
export interface RequestIssueItems {
  "cmd": "issue_items";
  "ref": GithubRef;
}

/**
 * Idempotent: the same link twice is `ok`; feature `github_links`.
 */
export interface RequestLinkIssue {
  "cmd": "link_issue";
  "item": QueueItemId;
  "ref": GithubRef;
  "relation": ManualLinkRelation;
}

export interface RequestUnlinkIssue {
  "cmd": "unlink_issue";
  "item": QueueItemId;
  "ref": GithubRef;
  "relation": ManualLinkRelation;
}

/**
 * Cached GitHub objects of one repository; feature `github_import`.
 */
export interface RequestListGithubObjects {
  "cmd": "list_github_objects";
  "kind"?: GithubKind | null;
  "limit"?: number | null;
  "repo": string;
  "state"?: GithubObjectState | null;
}

/**
 * Enqueue an issue as an item, keyed `github:<host>/<owner>/<repo>#<number>`
 * so a repeat returns the first item; feature `github_import`.
 */
export interface RequestImportIssue {
  "cmd": "import_issue";
  "pipeline"?: string | null;
  "priority"?: Priority | null;
  "ref": GithubRef;
}

export type Request =
  | RequestForAgent
  | RequestTracked
  | RequestRequestStatus
  | RequestMessageOrchestrator
  | RequestPublishArtifact
  | RequestListArtifacts
  | RequestReadArtifact
  | RequestHello
  | RequestStatusVariant
  | RequestGuide
  | RequestShutdown
  | RequestRepoAdd
  | RequestRepoList
  | RequestRepoRemove
  | RequestRepoUpdate
  | RequestEnqueue
  | RequestListItems
  | RequestListItemSummaries
  | RequestGetItem
  | RequestGetStage
  | RequestResolveItem
  | RequestUpdateItem
  | RequestCancelItem
  | RequestRetryItem
  | RequestRefreshItem
  | RequestPause
  | RequestResume
  | RequestSetCaps
  | RequestSettings
  | RequestSetSettings
  | RequestResetSettings
  | RequestListPipelines
  | RequestModelPolicy
  | RequestSay
  | RequestNotify
  | RequestClearNotifications
  | RequestAskOperator
  | RequestAnswer
  | RequestRecordChatAnswer
  | RequestListQuestions
  | RequestSubscribeFeed
  | RequestFeedHistory
  | RequestPublishReport
  | RequestListReports
  | RequestReopenReport
  | RequestOrchestratorStatus
  | RequestOrchestratorAttach
  | RequestOrchestratorRestart
  | RequestUpgrade
  | RequestCheckUpdates
  | RequestGc
  | RequestCostReport
  | RequestModelEvidence
  | RequestAutonomy
  | RequestSetDirective
  | RequestClearDirective
  | RequestGithubWatcher
  | RequestConfigureGithubWatcher
  | RequestGithubReply
  | RequestEnqueueInitiative
  | RequestListInitiatives
  | RequestCancelInitiative
  | RequestRetryInitiative
  | RequestGetInitiative
  | RequestUpdateInitiative
  | RequestSteerItem
  | RequestRecordLesson
  | RequestLessons
  | RequestUpdateLesson
  | RequestLessonsWhy
  | RequestLessonStats
  | RequestLearningCorrect
  | RequestLearningWhy
  | RequestLearningUndo
  | RequestLearningRestoreMemory
  | RequestLearningRetryMemory
  | RequestMergeItem
  | RequestItemDiff
  | RequestOperatorCommand
  | RequestUndoAuthority
  | RequestRequestAuthority
  | RequestPromote
  | RequestRequestMerge
  | RequestRemoteStatus
  | RequestConfigureRemote
  | RequestRemotePair
  | RequestRemoteRevoke
  | RequestSubscribeChanges
  | RequestChangesPage
  | RequestItemHistory
  | RequestTaskGraph
  | RequestRunPlan
  | RequestItemLinks
  | RequestIssueItems
  | RequestLinkIssue
  | RequestUnlinkIssue
  | RequestListGithubObjects
  | RequestImportIssue;

export type RequestId = string;

/**
 * What `request_status`, or a repeat under the same key, reports.
 */
export interface RequestStatus {
  "command": string;
  "error"?: ErrorReply | null;
  "finished_at"?: string | null;
  "idempotency_key"?: string | null;
  "reply"?: Reply | null;
  "request": RequestId;
  "started_at": string;
  "state": TrackedState;
}

export type Result_of_Reply_or_ErrorReply = Record<string, unknown>;

/**
 * GitHub's review decision on a pull request.
 */
export type ReviewDecision = "approved" | "changes_requested" | "review_required";

/**
 * Which review family a shipping item runs through.
 */
export type ReviewLevel = "strict" | "standard" | "none";

/**
 * How much human attention a pull request asks for before it merges.
 * Ordered: `Low < Medium < High`, so the higher of two calls is their `max`.
 */
export type RiskLevel = "low" | "medium" | "high";

export interface RouteContext {
  "review": string;
  "revision": string;
  "task_class": string;
}

/**
 * A run's frozen pipeline with the steer edits it ran under applied.
 */
export interface RunPlan {
  "edited": boolean;
  "pipeline": string;
  "revision": string;
  "run": PipelineRunId;
  "stages": Array<StageSummary>;
}

/**
 * Tagged by `state`.
 */
export interface RunStateRunning {
  "attempt": number;
  "stage_id": string;
  "state": "running";
}

/**
 * An operator gate (ARCHITECTURE §18.3): the stage passed and the run
 * waits for the operator to approve, revise or reject its output.
 */
export interface RunStateAwaitingOperator {
  "attempt": number;
  "stage_id": string;
  "state": "awaiting_operator";
  "subject": GateSubject;
}

export interface RunStateSucceeded {
  "state": "succeeded";
  "summary": string;
}

export interface RunStateFailed {
  "error": string;
  "stage_id": string;
  "state": "failed";
}

export interface RunStateCancelled {
  "reason"?: string | null;
  "state": "cancelled";
}

export type RunState =
  | RunStateRunning
  | RunStateAwaitingOperator
  | RunStateSucceeded
  | RunStateFailed
  | RunStateCancelled;

/**
 * A pipeline run's state without its data.
 */
export type RunStateKind = "running" | "awaiting_operator" | "succeeded" | "failed" | "cancelled";

/**
 * Tagged by `frame`.
 */
/**
 * `id: 0` is the daemon explaining why it is closing the connection.
 */
export interface ServerFrameReply {
  "frame": "reply";
  "id": number;
  "result": Result_of_Reply_or_ErrorReply;
}

export interface ServerFrameFeed {
  "frame": "feed";
  "item": FeedItem;
  "sub": number;
}

/**
 * One change of a `subscribe_changes` subscription.
 */
export interface ServerFrameChange {
  "change": Change;
  "frame": "change";
  "sub": number;
}

export type ServerFrame = ServerFrameReply | ServerFrameFeed | ServerFrameChange;

/**
 * Which layer decided an effective value.
 */
export type SettingSource = "builtin" | "config" | "global" | "registry" | "repo" | "repo_file";

/**
 * Model and effort as `provider/model effort`.
 */
export interface SettingsDrift {
  "configured": string;
  "running": string;
}

/**
 * `Reply::Settings`: what one scope stores, what lies beneath, and the
 * values in force with their sources.
 */
export interface SettingsView {
  "effective": EffectiveSettings;
  "global": OperatorSettings;
  "repo"?: string | null;
  "scoped": OperatorSettings;
}

/**
 * How finished work leaves a repository. Nothing merges unless
 * the item's `merge = auto` knob is within the operator's ceiling.
 */
export type ShipMode = "pr" | "local";

/**
 * How the cost of a stage run that delegated splits; the two amounts add up
 * to the stage run's `cost_usd`.
 */
export interface StageDelegation {
  "agents": Array<DelegatedAgent>;
  "delegated_usd": number;
  "direct_usd": number;
}

/**
 * `GetStage`'s answer: stage runs of the item's run asked for, oldest first.
 */
export interface StageDetail {
  "item": QueueItemId;
  "run": PipelineRunId;
  "stage"?: string | null;
  "stages": Array<StageRunView>;
}

export type StageKind = "work" | "review" | "test" | "ship" | "report" | "propose" | "team";

/**
 * Which stage runs of an item's run to show. Without `stage` it lists
 * them all; with it, the matching runs' output in full, paged.
 */
export interface StageQuery {
  "attempt"?: number | null;
  "item": QueueItemId;
  "limit"?: number | null;
  "offset"?: number;
  "run"?: PipelineRunId | null;
  "slot"?: number | null;
  "stage"?: string | null;
}

/**
 * One agent's worth of work: a stage, its attempt number, and for fan-out
 * stages which parallel slot it filled.
 */
export interface StageRun {
  "aether_agent"?: AetherAgentId | null;
  "aether_task"?: AetherTaskId | null;
  "attempt": number;
  "choice": ModelChoice;
  "cost_usd": number;
  "fan_index": number;
  "finished_at"?: string | null;
  "host_context"?: HostContext;
  "id": StageRunId;
  "infrastructure_failure"?: boolean;
  "kind"?: StageKind | null;
  "model_segments"?: Array<ModelSegment>;
  "run": PipelineRunId;
  "stage_id": string;
  "started_at": string;
  "state": StageState;
  "structured"?: unknown;
  "summary"?: string | null;
  "verdict"?: Verdict | null;
}

export type StageRunId = string;

export interface StageRunView {
  "aether_agent"?: AetherAgentId | null;
  "attempt": number;
  "choice": ModelChoice;
  "cost_usd": number;
  "delegation"?: StageDelegation | null;
  "output"?: TextPage | null;
  "slot": number;
  "stage_id": string;
  "state": StageState;
  "verdict"?: Verdict | null;
}

/**
 * Tagged by `state`.
 */
/**
 * Agent creation requested but not yet acknowledged by Aether.
 */
export interface StageStateSpawning {
  "state": "spawning";
}

export interface StageStateRunning {
  "state": "running";
}

export interface StageStateCompleted {
  "state": "completed";
}

export interface StageStateFailed {
  "error": string;
  "state": "failed";
}

export interface StageStateCancelled {
  "state": "cancelled";
}

/**
 * Wall-clock limit from the stage spec expired; Archon cancelled it.
 */
export interface StageStateTimedOut {
  "state": "timed_out";
}

export type StageState =
  | StageStateSpawning
  | StageStateRunning
  | StageStateCompleted
  | StageStateFailed
  | StageStateCancelled
  | StageStateTimedOut;

/**
 * A stage run's state without its data.
 */
export type StageStateKind = "spawning" | "running" | "completed" | "failed" | "cancelled" | "timed_out";

export interface StageSummary {
  "delegates_reviewers"?: boolean;
  "effort"?: Effort;
  "fan_out": number;
  "gate": Gate;
  "id": string;
  "kind": StageKind;
  "loops_to"?: string | null;
  "max_loops": number;
  "nudge"?: string | null;
  "roles"?: Array<string>;
  "roles_from"?: string | null;
  "tier"?: Tier | null;
  "timeout_minutes": number;
}

/**
 * An agent of a running stage that leaves inbox items unread: what a lead
 * that died while the agents it delegated to finished looks like.
 */
export interface StageWarning {
  "agent": AetherAgentId;
  "item": QueueItemId;
  "since": string;
  "stage": StageRunId;
  "stage_id": string;
  "unread": number;
}

/**
 * `DaemonStatus::health`. archond asks Aether off the request path and
 * shows its latest answers, so status never waits on a slow aetherd.
 */
export interface StatusHealth {
  "aether": ProbeHealth;
  "github"?: ProbeHealth | null;
  "mcp": ProbeHealth;
}

/**
 * `Status`'s page of waiting items. The default encodes as a bare
 * `{"cmd":"status"}`, which every archond reads.
 */
export interface StatusQuery {
  "waiting_limit"?: number | null;
  "waiting_offset"?: number;
}

export interface Steer {
  "abort"?: Then | null;
  "autonomy"?: AutonomyPatch | null;
  "budget_usd"?: number | null;
  "effort"?: Effort | null;
  "extend_minutes"?: number | null;
  "insert_stage"?: InsertStage | null;
  "model"?: ModelRef | null;
  "note"?: string | null;
  "stage"?: string | null;
  "stage_timeouts"?: Record<string, number>;
  "switch_pipeline"?: string | null;
}

/**
 * One steer in an item's history.
 */
export interface SteerRecord {
  "at": string;
  "by": SteeredBy;
  "note"?: string | null;
  "what": string;
}

/**
 * Who may steer an item (ARCHITECTURE §6.6).
 */
export type SteeredBy = "operator" | "orchestrator";

/**
 * Allocated bytes for one storage kind, without following symlinks.
 */
export interface StorageUsage {
  "bytes"?: number | null;
  "error"?: string | null;
  "kind": string;
  "path": string;
}

/**
 * One structural limit as in force, for `archon autonomy` and the
 * `autonomy_policy` tool.
 */
export interface StructuralLimit {
  "ceiling": number;
  "default": number;
  "key": string;
  "source": LimitSource;
  "value": number;
}

/**
 * `from` is the prerequisite and `to` the item that waits for it.
 */
export interface TaskEdge {
  "from": QueueItemId;
  "kind": TaskEdgeKind;
  "satisfied": boolean;
  "to": QueueItemId;
}

export type TaskEdgeKind = "depends_on" | "initiative_after" | "stacked_on";

/**
 * Which items a `task_graph` holds; an absent field does not filter.
 */
export interface TaskFilter {
  "component_of"?: QueueItemId | null;
  "include_terminal_since"?: string | null;
  "initiative"?: InitiativeId | null;
  "min_priority"?: Priority | null;
  "pipelines"?: Array<string> | null;
  "repos"?: Array<RepoId> | null;
  "states"?: Array<QueueStateKind> | null;
}

/**
 * Nodes ordered by `(created_at, id)` and edges by `(from, to, kind)`, so
 * the same store gives the same bytes. `as_of` is the last change seq
 * committed in the read transaction the rows came from.
 */
export interface TaskGraph {
  "as_of": number;
  "edges": Array<TaskEdge>;
  "initiatives": Array<GraphInitiative>;
  "nodes": Array<TaskNode>;
  "truncated": boolean;
}

/**
 * One item in the graph: a subset of its summary.
 */
export interface TaskNode {
  "agents_running": number;
  "created_at": string;
  "id": QueueItemId;
  "initiative"?: InitiativeId | null;
  "issue"?: GithubRef | null;
  "pipeline": string;
  "pr"?: PrRef | null;
  "priority": Priority;
  "repo"?: RepoId | null;
  "spent_usd": number;
  "stage"?: string | null;
  "state": QueueStateKind;
  "state_detail"?: string | null;
  "tags": Array<string>;
  "title": string;
  "updated_at": string;
  "waiting_reason"?: WaitingReasonCode | null;
}

/**
 * Characters `offset..offset + text.chars().count()` of a longer text.
 */
export interface TextPage {
  "offset": number;
  "text": string;
  "total_chars": number;
}

/**
 * What the run does once a stage was aborted.
 * Tagged by `then`.
 */
export interface ThenRetry {
  "then": "retry";
}

export interface ThenSkip {
  "then": "skip";
}

export interface ThenGoto {
  "then": "goto";
  "to": string;
}

export type Then = ThenRetry | ThenSkip | ThenGoto;

/**
 * A provider-local capability band, weakest first.
 */
export type Tier = "fast" | "balanced" | "frontier" | "strong" | "strongest";

/**
 * How a plan entry asked by tier was resolved; recorded on the plan and on
 * every attempt that runs it.
 */
export interface TierChoice {
  "provider"?: string | null;
  "requested": Tier;
  "resolved": Tier;
}

/**
 * The same observations pooled by the tier of the model that ran, so they
 * outlive a model replaced within its tier.
 */
export interface TierEvidenceRow {
  "attempts": number;
  "completed": number;
  "effort": Effort;
  "median_cost_usd"?: number | null;
  "median_minutes"?: number | null;
  "p90_cost_usd"?: number | null;
  "passed": number;
  "stage": string;
  "tier": Tier;
  "verdicts": number;
}

/**
 * A patch to the registry's [`RepoTiers`], field by field.
 */
export interface TiersPatch {
  "confirmed_fast_check"?: Clearable;
  "confirmed_full_check"?: Clearable;
  "confirmed_release_check"?: Clearable;
  "fast_check"?: Clearable;
  "full_check"?: Clearable;
  "integration_branch"?: Clearable;
  "release_branch"?: Clearable;
  "release_hook"?: Clearable;
}

/**
 * How much of Aether's tool surface Archon's agents get.
 */
export type ToolsMode = "guarded" | "yolo";

/**
 * Where a tracked request stands.
 */
export type TrackedState = "pending" | "applied" | "failed" | "interrupted";

/**
 * Who made an item transition.
 */
export type TransitionActor = "daemon" | "cli" | "web" | "mcp" | "github";

/**
 * An operator's upgrade: drain, install what is named, restart archond
 * and, in dedicated mode, Archon's own aetherd. With neither program
 * named it is a drained restart.
 */
export interface UpgradeRequest {
  "aether"?: UpgradeTarget | null;
  "archon"?: UpgradeTarget | null;
  "drain_secs": number;
  "keep_aether"?: boolean;
  "requeue"?: boolean;
}

/**
 * What an upgrade installs for one program.
 */
export type UpgradeTarget = "latest" | "main" | Record<string, unknown>;

/**
 * What a review/test stage concluded, as [`Verdict::extract`] reads it from
 * the stage agent's result; anything unreadable is `Unknown` and fails a gate.
 */
export type Verdict = "pass" | "fail" | "unknown";

/**
 * Archon's and Aether's versions, for `status` and `queue_status`.
 */
export interface Versions {
  "aether": ProgramVersions;
  "archon": ProgramVersions;
}

/**
 * What the current run needs from the operator, and how to provide it.
 */
export interface WaitingOn {
  "next_action": string;
  "prompt": string;
  "question"?: QuestionId | null;
  "stage": string;
  "subject": string;
}

export interface WaitingReason {
  "code": WaitingReasonCode;
  "message": string;
  "next_action": string;
}

/**
 * Stable read-time reason codes; these are never stored on a queue item.
 */
export type WaitingReasonCode = "aether_disconnected" | "operator_gate" | "dependency_failed" | "dependency_waiting" | "global_pause" | "low_disk" | "total_budget" | "daily_budget" | "repo_pause" | "pipeline_pause" | "capacity" | "next_in_line" | "draining" | "host_load" | "blocked" | "provider_quota";

/**
 * Where a watched repository's requests go.
 * Tagged by `kind`.
 */
/**
 * Archon's: fixed dispatch to the current orchestrator.
 */
export interface WatchTargetOrchestrator {
  "kind": "orchestrator";
}

/**
 * Archon's, but aimed elsewhere (an old orchestrator); archond repoints it.
 */
export interface WatchTargetStale {
  "agent"?: AetherAgentId | null;
  "kind": "stale";
}

/**
 * Another dispatch mode, set through `archon aether github-watcher`;
 * archond leaves it alone and `github_reply` refuses its threads.
 */
export interface WatchTargetUnmanaged {
  "kind": "unmanaged";
  "mode": string;
}

export type WatchTarget = WatchTargetOrchestrator | WatchTargetStale | WatchTargetUnmanaged;

export interface WatchedRepo {
  "allowed_users"?: Array<string> | null;
  "dispatched": number;
  "enabled": boolean;
  "excluded_operators"?: Array<string>;
  "ignored": number;
  "last_error"?: string | null;
  "last_poll_at"?: string | null;
  "repository": string;
  "target": WatchTarget;
}

/**
 * An operator's edit to Archon's watcher. Nothing here names a target:
 * archond always points Archon's repositories at the live orchestrator.
 * Tagged by `action`.
 */
/**
 * Watch a repository (and switch the watcher on). An allowlist replaces
 * the repository's current one in the same write; `None` keeps it.
 */
export interface WatcherChangeAdd {
  "action": "add";
  "allowed_users"?: AllowedUsers | null;
  "repository": GithubRepo;
}

/**
 * Replace a watched repository's allowlist, or clear it with `None` so
 * every collaborator with write access may dispatch again.
 */
export interface WatcherChangeAllow {
  "action": "allow";
  "allowed_users"?: AllowedUsers | null;
  "repository": GithubRepo;
}

/**
 * Replace the watcher's operator logins, or clear them with `None` so
 * that every GitHub request is an external request again.
 */
export interface WatcherChangeOperators {
  "action": "operators";
  "operator_users"?: OperatorUsers | null;
}

export interface WatcherChangeRemove {
  "action": "remove";
  "repository": GithubRepo;
}

/**
 * One repository, or the whole watcher when `None`.
 */
export interface WatcherChangeEnable {
  "action": "enable";
  "repository"?: GithubRepo | null;
}

export interface WatcherChangeDisable {
  "action": "disable";
  "repository"?: GithubRepo | null;
}

/**
 * `None` keeps the current setting.
 */
export interface WatcherChangeConfigure {
  "action": "configure";
  "label_regex"?: string | null;
  "mention_regex"?: string | null;
  "poll_interval_secs"?: number | null;
}

export type WatcherChange =
  | WatcherChangeAdd
  | WatcherChangeAllow
  | WatcherChangeOperators
  | WatcherChangeRemove
  | WatcherChangeEnable
  | WatcherChangeDisable
  | WatcherChangeConfigure;

export interface Welcome {
  "daemon_version": string;
  "device"?: WelcomeDevice | null;
  "features"?: Array<string>;
  "min_protocol_version": number;
  "pid": number;
  "protocol_version": number;
}

/**
 * A remote session's device, as its `welcome` names it.
 */
export interface WelcomeDevice {
  "id": string;
  "name": string;
  "role": DeviceRole;
}

export type WorkspaceMode = "worktree" | "repo-readonly" | "scratch";

/**
 * The paths in `path`, a worktree, that a gc preview found it would delete.
 */
export interface WouldDelete {
  "path": string;
  "paths": Array<string>;
}

// Commands. Each builds one frame; the client adds the request id.

/**
 * CLI only: the operator's answer, never the orchestrator's.
 */
export interface AnswerCommand {
  "cmd": "answer";
  "chat"?: boolean;
  "question": QuestionId;
  "text": string;
}

export function answer(args: Omit<AnswerCommand, "cmd">): AnswerCommand {
  return { cmd: "answer", ...args };
}

export interface AskOperatorCommand {
  "cmd": "ask_operator";
  "caller"?: AetherAgentId | null;
  "item"?: QueueItemId | null;
  "options"?: Array<string>;
  "text": string;
}

export function askOperator(args: Omit<AskOperatorCommand, "cmd">): AskOperatorCommand {
  return { cmd: "ask_operator", ...args };
}

/**
 * Autonomy defaults, ceiling and directives, for one repository or
 * globally (ARCHITECTURE §18).
 */
export interface AutonomyCommand {
  "cmd": "autonomy";
  "repo"?: RepoRef | null;
}

export function autonomy(args: Omit<AutonomyCommand, "cmd">): AutonomyCommand {
  return { cmd: "autonomy", ...args };
}

export interface CancelInitiativeCommand {
  "cmd": "cancel_initiative";
  "initiative": InitiativeId;
}

export function cancelInitiative(args: Omit<CancelInitiativeCommand, "cmd">): CancelInitiativeCommand {
  return { cmd: "cancel_initiative", ...args };
}

export interface CancelItemCommand {
  "cmd": "cancel_item";
  "item": QueueItemId;
  "reason"?: string | null;
}

export function cancelItem(args: Omit<CancelItemCommand, "cmd">): CancelItemCommand {
  return { cmd: "cancel_item", ...args };
}

/**
 * The changes with `seq > after`, oldest first; feature `changes`.
 */
export interface ChangesPageCommand {
  "cmd": "changes_page";
  "after": number;
  "limit"?: number | null;
}

export function changesPage(args: Omit<ChangesPageCommand, "cmd">): ChangesPageCommand {
  return { cmd: "changes_page", ...args };
}

/**
 * Operator only, as it asks the network: the latest releases of archon
 * and aether, kept for `status` and `queue_status`.
 */
export interface CheckUpdatesCommand {
  "cmd": "check_updates";
}

export function checkUpdates(): CheckUpdatesCommand {
  return { cmd: "check_updates" };
}

/**
 * Clear one directive, or every directive when `None`.
 */
export interface ClearDirectiveCommand {
  "cmd": "clear_directive";
  "directive"?: DirectiveId | null;
}

export function clearDirective(args: Omit<ClearDirectiveCommand, "cmd">): ClearDirectiveCommand {
  return { cmd: "clear_directive", ...args };
}

/**
 * Resolve the notifications archond posted to Aether's notification
 * center: the open blockers of one item, or every open blocker and
 * outcome when `item` is `None`.
 */
export interface ClearNotificationsCommand {
  "cmd": "clear_notifications";
  "item"?: QueueItemId | null;
}

export function clearNotifications(args: Omit<ClearNotificationsCommand, "cmd">): ClearNotificationsCommand {
  return { cmd: "clear_notifications", ...args };
}

/**
 * CLI only: GitHub content must never change what is watched.
 */
export interface ConfigureGithubWatcherCommand {
  "cmd": "configure_github_watcher";
  "change": WatcherChange;
}

export function configureGithubWatcher(args: Omit<ConfigureGithubWatcherCommand, "cmd">): ConfigureGithubWatcherCommand {
  return { cmd: "configure_github_watcher", ...args };
}

export interface ConfigureRemoteCommand {
  "cmd": "configure_remote";
  "change": RemoteChange;
}

export function configureRemote(args: Omit<ConfigureRemoteCommand, "cmd">): ConfigureRemoteCommand {
  return { cmd: "configure_remote", ...args };
}

/**
 * Spend over a range of local days, grouped by day, repository,
 * pipeline, model, item or stage (ARCHITECTURE §5.3).
 */
export interface CostReportCommand {
  "cmd": "cost_report";
  "by"?: CostGroup;
  "since"?: DayBound | null;
  "until"?: DayBound | null;
}

export function costReport(args: Omit<CostReportCommand, "cmd">): CostReportCommand {
  return { cmd: "cost_report", ...args };
}

export interface EnqueueCommand {
  "cmd": "enqueue";
  "autonomy"?: AutonomyPatch;
  "brief": string;
  "budget_usd"?: number | null;
  "depends_on"?: Array<QueueItemId>;
  "idempotency_key"?: string | null;
  "model_plan"?: ModelPlanDraft;
  "model_reasons"?: Record<string, string> | null;
  "onto"?: OntoTarget | null;
  "origin"?: GithubThread | null;
  "pipeline"?: string | null;
  "priority"?: Priority;
  "repo"?: RepoRef | null;
  "stage_timeouts"?: Record<string, number>;
  "tags"?: Array<string>;
  "title": string;
}

export function enqueue(args: Omit<EnqueueCommand, "cmd">): EnqueueCommand {
  return { cmd: "enqueue", ...args };
}

/**
 * Several items, usually in several repositories, built against one
 * shared contract.
 */
export interface EnqueueInitiativeCommand {
  "cmd": "enqueue_initiative";
  "atomic"?: boolean;
  "contract": string;
  "idempotency_key"?: string | null;
  "members": Array<InitiativeMember>;
  "title": string;
}

export function enqueueInitiative(args: Omit<EnqueueInitiativeCommand, "cmd">): EnqueueInitiativeCommand {
  return { cmd: "enqueue_initiative", ...args };
}

export interface FeedHistoryCommand {
  "cmd": "feed_history";
  "before"?: number | null;
  "limit": number;
  "verbose": boolean;
}

export function feedHistory(args: Omit<FeedHistoryCommand, "cmd">): FeedHistoryCommand {
  return { cmd: "feed_history", ...args };
}

/**
 * A tool call `archon mcp` forwards for the Aether agent that made it:
 * only the orchestrator of now may change Archon (ARCHITECTURE §11).
 */
export interface ForAgentCommand {
  "cmd": "for_agent";
  "caller": AetherAgentId;
  "request": Request;
}

export function forAgent(args: Omit<ForAgentCommand, "cmd">): ForAgentCommand {
  return { cmd: "for_agent", ...args };
}

/**
 * CLI only: collect what finished runs left (ARCHITECTURE §13.2).
 * Worktrees go once their runs finished `older_than_secs` ago
 * (`None`: `worktrees.gc_after`); with `force`, even with changes.
 */
export interface GcCommand {
  "cmd": "gc";
  "force": boolean;
  "older_than_secs"?: number | null;
  "preview"?: boolean;
}

export function gc(args: Omit<GcCommand, "cmd">): GcCommand {
  return { cmd: "gc", ...args };
}

export interface GetInitiativeCommand {
  "cmd": "get_initiative";
  "initiative": InitiativeId;
}

export function getInitiative(args: Omit<GetInitiativeCommand, "cmd">): GetInitiativeCommand {
  return { cmd: "get_initiative", ...args };
}

export interface GetItemCommand {
  "cmd": "get_item";
  "item": QueueItemId;
}

export function getItem(args: Omit<GetItemCommand, "cmd">): GetItemCommand {
  return { cmd: "get_item", ...args };
}

/**
 * The stage runs of an item's latest run, or one stage's full output.
 */
export interface GetStageCommand {
  "cmd": "get_stage";
  "attempt"?: number | null;
  "item": QueueItemId;
  "limit"?: number | null;
  "offset"?: number;
  "run"?: PipelineRunId | null;
  "slot"?: number | null;
  "stage"?: string | null;
}

export function getStage(args: Omit<GetStageCommand, "cmd">): GetStageCommand {
  return { cmd: "get_stage", ...args };
}

/**
 * Comment on a thread of a repository Archon watches; archond appends
 * the reply marker so the comment never re-triggers the watcher.
 */
export interface GithubReplyCommand {
  "cmd": "github_reply";
  "body": string;
  "thread": GithubThread;
}

export function githubReply(args: Omit<GithubReplyCommand, "cmd">): GithubReplyCommand {
  return { cmd: "github_reply", ...args };
}

/**
 * Archon's GitHub watcher in its aetherd.
 */
export interface GithubWatcherCommand {
  "cmd": "github_watcher";
}

export function githubWatcher(): GithubWatcherCommand {
  return { cmd: "github_watcher" };
}

export interface GuideCommand {
  "cmd": "guide";
  "topic": string;
}

export function guide(args: Omit<GuideCommand, "cmd">): GuideCommand {
  return { cmd: "guide", ...args };
}

export interface HelloCommand {
  "cmd": "hello";
  "client": ClientKind;
  "client_version": string;
  "features"?: Array<string>;
  "protocol_version": number;
}

export function hello(args: Omit<HelloCommand, "cmd">): HelloCommand {
  return { cmd: "hello", ...args };
}

/**
 * Enqueue an issue as an item, keyed `github:<host>/<owner>/<repo>#<number>`
 * so a repeat returns the first item; feature `github_import`.
 */
export interface ImportIssueCommand {
  "cmd": "import_issue";
  "pipeline"?: string | null;
  "priority"?: Priority | null;
  "ref": GithubRef;
}

export function importIssue(args: Omit<ImportIssueCommand, "cmd">): ImportIssueCommand {
  return { cmd: "import_issue", ...args };
}

/**
 * The items linked to one issue or pull request; feature `github_links`.
 */
export interface IssueItemsCommand {
  "cmd": "issue_items";
  "ref": GithubRef;
}

export function issueItems(args: Omit<IssueItemsCommand, "cmd">): IssueItemsCommand {
  return { cmd: "issue_items", ...args };
}

/**
 * What a `ship = local` item's branch changes against its base, and
 * whether `MergeItem` would land it now.
 */
export interface ItemDiffCommand {
  "cmd": "item_diff";
  "full"?: boolean;
  "item": QueueItemId;
}

export function itemDiff(args: Omit<ItemDiffCommand, "cmd">): ItemDiffCommand {
  return { cmd: "item_diff", ...args };
}

/**
 * An item's state transitions, oldest first; feature `item_history`.
 */
export interface ItemHistoryCommand {
  "cmd": "item_history";
  "item": QueueItemId;
  "limit"?: number | null;
}

export function itemHistory(args: Omit<ItemHistoryCommand, "cmd">): ItemHistoryCommand {
  return { cmd: "item_history", ...args };
}

/**
 * An item's GitHub links and their cached objects; feature `github_links`.
 */
export interface ItemLinksCommand {
  "cmd": "item_links";
  "item": QueueItemId;
}

export function itemLinks(args: Omit<ItemLinksCommand, "cmd">): ItemLinksCommand {
  return { cmd: "item_links", ...args };
}

/**
 * CLI only: the operator corrects one stage of a finished run.
 */
export interface LearningCorrectCommand {
  "cmd": "learning_correct";
  "cause": CorrectionCause;
  "forget_memory"?: AetherMemoryId | null;
  "item": QueueItemId;
  "reason": string;
  "run": PipelineRunId;
  "stage": string;
}

export function learningCorrect(args: Omit<LearningCorrectCommand, "cmd">): LearningCorrectCommand {
  return { cmd: "learning_correct", ...args };
}

/**
 * CLI only: restore the memory a correction forgot.
 */
export interface LearningRestoreMemoryCommand {
  "cmd": "learning_restore_memory";
  "feedback": FeedbackId;
}

export function learningRestoreMemory(args: Omit<LearningRestoreMemoryCommand, "cmd">): LearningRestoreMemoryCommand {
  return { cmd: "learning_restore_memory", ...args };
}

/**
 * CLI only: retry a correction's pending or failed memory forget.
 */
export interface LearningRetryMemoryCommand {
  "cmd": "learning_retry_memory";
  "feedback": FeedbackId;
}

export function learningRetryMemory(args: Omit<LearningRetryMemoryCommand, "cmd">): LearningRetryMemoryCommand {
  return { cmd: "learning_retry_memory", ...args };
}

/**
 * CLI only: revoke a correction's model penalty; its memory stays as is.
 */
export interface LearningUndoCommand {
  "cmd": "learning_undo";
  "feedback": FeedbackId;
}

export function learningUndo(args: Omit<LearningUndoCommand, "cmd">): LearningUndoCommand {
  return { cmd: "learning_undo", ...args };
}

/**
 * The corrections of an item's runs and what each does now.
 */
export interface LearningWhyCommand {
  "cmd": "learning_why";
  "item": QueueItemId;
}

export function learningWhy(args: Omit<LearningWhyCommand, "cmd">): LearningWhyCommand {
  return { cmd: "learning_why", ...args };
}

export interface LessonStatsCommand {
  "cmd": "lesson_stats";
  "repo"?: RepoRef | null;
}

export function lessonStats(args: Omit<LessonStatsCommand, "cmd">): LessonStatsCommand {
  return { cmd: "lesson_stats", ...args };
}

export interface LessonsCommand {
  "cmd": "lessons";
  "kind"?: LessonKind | null;
  "repo"?: RepoRef | null;
  "state"?: LessonState | null;
}

export function lessons(args: Omit<LessonsCommand, "cmd">): LessonsCommand {
  return { cmd: "lessons", ...args };
}

/**
 * The lessons and learned defaults that shaped an item.
 */
export interface LessonsWhyCommand {
  "cmd": "lessons_why";
  "item": QueueItemId;
}

export function lessonsWhy(args: Omit<LessonsWhyCommand, "cmd">): LessonsWhyCommand {
  return { cmd: "lessons_why", ...args };
}

/**
 * Idempotent: the same link twice is `ok`; feature `github_links`.
 */
export interface LinkIssueCommand {
  "cmd": "link_issue";
  "item": QueueItemId;
  "ref": GithubRef;
  "relation": ManualLinkRelation;
}

export function linkIssue(args: Omit<LinkIssueCommand, "cmd">): LinkIssueCommand {
  return { cmd: "link_issue", ...args };
}

export interface ListArtifactsCommand {
  "cmd": "list_artifacts";
  "item": QueueItemId;
}

export function listArtifacts(args: Omit<ListArtifactsCommand, "cmd">): ListArtifactsCommand {
  return { cmd: "list_artifacts", ...args };
}

/**
 * Cached GitHub objects of one repository; feature `github_import`.
 */
export interface ListGithubObjectsCommand {
  "cmd": "list_github_objects";
  "kind"?: GithubKind | null;
  "limit"?: number | null;
  "repo": string;
  "state"?: GithubObjectState | null;
}

export function listGithubObjects(args: Omit<ListGithubObjectsCommand, "cmd">): ListGithubObjectsCommand {
  return { cmd: "list_github_objects", ...args };
}

export interface ListInitiativesCommand {
  "cmd": "list_initiatives";
}

export function listInitiatives(): ListInitiativesCommand {
  return { cmd: "list_initiatives" };
}

/**
 * Like `ListItems`, each item with its live stage, spend and PR URL.
 */
export interface ListItemSummariesCommand {
  "cmd": "list_item_summaries";
  "before"?: string | null;
  "limit"?: number | null;
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "states"?: Array<string>;
  "tag"?: string | null;
  "text"?: string | null;
  "updated_since"?: string | null;
  "with_as_of"?: boolean;
}

export function listItemSummaries(args: Omit<ListItemSummariesCommand, "cmd">): ListItemSummariesCommand {
  return { cmd: "list_item_summaries", ...args };
}

export interface ListItemsCommand {
  "cmd": "list_items";
  "before"?: string | null;
  "limit"?: number | null;
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "states"?: Array<string>;
  "tag"?: string | null;
  "text"?: string | null;
  "updated_since"?: string | null;
  "with_as_of"?: boolean;
}

export function listItems(args: Omit<ListItemsCommand, "cmd">): ListItemsCommand {
  return { cmd: "list_items", ...args };
}

export interface ListPipelinesCommand {
  "cmd": "list_pipelines";
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
}

export function listPipelines(args: Omit<ListPipelinesCommand, "cmd">): ListPipelinesCommand {
  return { cmd: "list_pipelines", ...args };
}

export interface ListQuestionsCommand {
  "cmd": "list_questions";
  "open_only": boolean;
}

export function listQuestions(args: Omit<ListQuestionsCommand, "cmd">): ListQuestionsCommand {
  return { cmd: "list_questions", ...args };
}

/**
 * Every recorded report, most recently changed first.
 */
export interface ListReportsCommand {
  "cmd": "list_reports";
}

export function listReports(): ListReportsCommand {
  return { cmd: "list_reports" };
}

/**
 * CLI only: the operator merges an item's delivered pull request now,
 * once archond's checks pass (ARCHITECTURE §18.6).
 */
export interface MergeItemCommand {
  "cmd": "merge_item";
  "chat"?: boolean;
  "ignore_checks"?: boolean;
  "item": QueueItemId;
}

export function mergeItem(args: Omit<MergeItemCommand, "cmd">): MergeItemCommand {
  return { cmd: "merge_item", ...args };
}

export interface MessageOrchestratorCommand {
  "cmd": "message_orchestrator";
  "caller"?: AetherAgentId | null;
  "kind"?: MessageKind;
  "text": string;
}

export function messageOrchestrator(args: Omit<MessageOrchestratorCommand, "cmd">): MessageOrchestratorCommand {
  return { cmd: "message_orchestrator", ...args };
}

export interface ModelEvidenceCommand {
  "cmd": "model_evidence";
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "since"?: DayBound | null;
  "stage"?: string | null;
}

export function modelEvidence(args: Omit<ModelEvidenceCommand, "cmd">): ModelEvidenceCommand {
  return { cmd: "model_evidence", ...args };
}

export interface ModelPolicyCommand {
  "cmd": "model_policy";
  "repo"?: RepoRef | null;
}

export function modelPolicy(args: Omit<ModelPolicyCommand, "cmd">): ModelPolicyCommand {
  return { cmd: "model_policy", ...args };
}

export interface NotifyCommand {
  "cmd": "notify";
  "item"?: QueueItemId | null;
  "kind": NotificationKind;
  "text": string;
}

export function notify(args: Omit<NotifyCommand, "cmd">): NotifyCommand {
  return { cmd: "notify", ...args };
}

/**
 * Whole-message command on the operator socket; never exposed over MCP.
 */
export interface OperatorCommandCommand {
  "cmd": "operator_command";
  "text": string;
}

export function operatorCommand(args: Omit<OperatorCommandCommand, "cmd">): OperatorCommandCommand {
  return { cmd: "operator_command", ...args };
}

/**
 * CLI only: ensure the orchestrator is live, without sending a message.
 */
export interface OrchestratorAttachCommand {
  "cmd": "orchestrator_attach";
}

export function orchestratorAttach(): OrchestratorAttachCommand {
  return { cmd: "orchestrator_attach" };
}

/**
 * CLI only: cancel and recreate the orchestrator agent.
 */
export interface OrchestratorRestartCommand {
  "cmd": "orchestrator_restart";
}

export function orchestratorRestart(): OrchestratorRestartCommand {
  return { cmd: "orchestrator_restart" };
}

export interface OrchestratorStatusCommand {
  "cmd": "orchestrator_status";
}

export function orchestratorStatus(): OrchestratorStatusCommand {
  return { cmd: "orchestrator_status" };
}

export interface PauseCommand {
  "cmd": "pause";
}

export function pause(): PauseCommand {
  return { cmd: "pause" };
}

/**
 * Promote the repository's integration branch into its release branch
 * once its head is green; an agent's call merges only under `grant_merge`.
 */
export interface PromoteCommand {
  "cmd": "promote";
  "repo": RepoRef;
}

export function promote(args: Omit<PromoteCommand, "cmd">): PromoteCommand {
  return { cmd: "promote", ...args };
}

/**
 * A stage agent keeps files from its workspace in its item's artifact
 * store (ARCHITECTURE §13.5).
 */
export interface PublishArtifactCommand {
  "cmd": "publish_artifact";
  "caller"?: AetherAgentId | null;
  "name"?: string | null;
  "path": string;
}

export function publishArtifact(args: Omit<PublishArtifactCommand, "cmd">): PublishArtifactCommand {
  return { cmd: "publish_artifact", ...args };
}

export interface PublishReportCommand {
  "cmd": "publish_report";
  "item"?: QueueItemId | null;
  "path": string;
  "title": string;
}

export function publishReport(args: Omit<PublishReportCommand, "cmd">): PublishReportCommand {
  return { cmd: "publish_report", ...args };
}

export interface ReadArtifactCommand {
  "cmd": "read_artifact";
  "item": QueueItemId;
  "limit"?: number | null;
  "name": string;
  "offset"?: number;
}

export function readArtifact(args: Omit<ReadArtifactCommand, "cmd">): ReadArtifactCommand {
  return { cmd: "read_artifact", ...args };
}

/**
 * The orchestrator records that the operator answered, in chat, a
 * question it asked with `ask_operator`. Never a gate's question.
 */
export interface RecordChatAnswerCommand {
  "cmd": "record_chat_answer";
  "answer": string;
  "caller"?: AetherAgentId | null;
  "question": QuestionId;
}

export function recordChatAnswer(args: Omit<RecordChatAnswerCommand, "cmd">): RecordChatAnswerCommand {
  return { cmd: "record_chat_answer", ...args };
}

export interface RecordLessonCommand {
  "cmd": "record_lesson";
  "item"?: QueueItemId | null;
  "kind": LessonKind;
  "pipeline"?: string | null;
  "repo"?: RepoRef | null;
  "text": string;
}

export function recordLesson(args: Omit<RecordLessonCommand, "cmd">): RecordLessonCommand {
  return { cmd: "record_lesson", ...args };
}

/**
 * Merge the finished item's base branch, fetched afresh, into its branch:
 * a new item on the same branch and pull request (ARCHITECTURE §13).
 * Its plan and stage timeouts are the stage defaults unless given here,
 * never the finished item's own.
 */
export interface RefreshItemCommand {
  "cmd": "refresh_item";
  "item": QueueItemId;
  "model_plan"?: ModelPlanDraft | null;
  "model_reasons"?: Record<string, string> | null;
  "stage_timeouts"?: Record<string, number>;
}

export function refreshItem(args: Omit<RefreshItemCommand, "cmd">): RefreshItemCommand {
  return { cmd: "refresh_item", ...args };
}

/**
 * Open a pairing with a new one-time code, replacing any open one.
 */
export interface RemotePairCommand {
  "cmd": "remote_pair";
  "role"?: DeviceRole | null;
}

export function remotePair(args: Omit<RemotePairCommand, "cmd">): RemotePairCommand {
  return { cmd: "remote_pair", ...args };
}

/**
 * Unpair a device by id or name, closing its sessions at once.
 */
export interface RemoteRevokeCommand {
  "cmd": "remote_revoke";
  "device": string;
}

export function remoteRevoke(args: Omit<RemoteRevokeCommand, "cmd">): RemoteRevokeCommand {
  return { cmd: "remote_revoke", ...args };
}

/**
 * Remote access (docs/remote.md): the listener, paired devices and any
 * open pairing. Operator only, and never served to the web app itself.
 */
export interface RemoteStatusCommand {
  "cmd": "remote_status";
}

export function remoteStatus(): RemoteStatusCommand {
  return { cmd: "remote_status" };
}

/**
 * Show a recorded report again at the operator's request: resume its
 * Lavish session, even one they ended, or name its path without
 * Lavish. A failed reopen leaves the report as it was.
 */
export interface ReopenReportCommand {
  "cmd": "reopen_report";
  "report": ReportId;
}

export function reopenReport(args: Omit<ReopenReportCommand, "cmd">): ReopenReportCommand {
  return { cmd: "reopen_report", ...args };
}

export interface RepoAddCommand {
  "cmd": "repo_add";
  "auto_merge"?: boolean;
  "base_branch"?: string | null;
  "default_pipeline"?: string | null;
  "github_watch"?: boolean;
  "name"?: string | null;
  "path": string;
  "ship"?: ShipMode | null;
}

export function repoAdd(args: Omit<RepoAddCommand, "cmd">): RepoAddCommand {
  return { cmd: "repo_add", ...args };
}

export interface RepoListCommand {
  "cmd": "repo_list";
}

export function repoList(): RepoListCommand {
  return { cmd: "repo_list" };
}

/**
 * CLI only.
 */
export interface RepoRemoveCommand {
  "cmd": "repo_remove";
  "repo": RepoRef;
}

export function repoRemove(args: Omit<RepoRemoveCommand, "cmd">): RepoRemoveCommand {
  return { cmd: "repo_remove", ...args };
}

/**
 * CLI only; the one path to a repository's autonomy ceiling.
 */
export interface RepoUpdateCommand {
  "cmd": "repo_update";
  "patch": RepoPatch;
  "repo": RepoRef;
}

export function repoUpdate(args: Omit<RepoUpdateCommand, "cmd">): RepoUpdateCommand {
  return { cmd: "repo_update", ...args };
}

/**
 * The orchestrator cites a message observed on archond's own stream.
 */
export interface RequestAuthorityCommand {
  "cmd": "request_authority";
  "action": Request;
  "caller"?: AetherAgentId | null;
  "quote": string;
  "reason": string;
  "utterance": number;
}

export function requestAuthority(args: Omit<RequestAuthorityCommand, "cmd">): RequestAuthorityCommand {
  return { cmd: "request_authority", ...args };
}

/**
 * Request a merge; archond confirms unless `grant_merge` permits it.
 */
export interface RequestMergeCommand {
  "cmd": "request_merge";
  "item": QueueItemId;
  "reason"?: string | null;
}

export function requestMerge(args: Omit<RequestMergeCommand, "cmd">): RequestMergeCommand {
  return { cmd: "request_merge", ...args };
}

/**
 * What became of a tracked change the caller made.
 */
export interface RequestStatusCommand {
  "cmd": "request_status";
  "request": RequestId;
}

export function requestStatus(args: Omit<RequestStatusCommand, "cmd">): RequestStatusCommand {
  return { cmd: "request_status", ...args };
}

/**
 * Operator only: forget the overlay stored at one scope.
 */
export interface ResetSettingsCommand {
  "cmd": "reset_settings";
  "repo"?: RepoRef | null;
}

export function resetSettings(args: Omit<ResetSettingsCommand, "cmd">): ResetSettingsCommand {
  return { cmd: "reset_settings", ...args };
}

/**
 * The one item whose id starts with `prefix` (see
 * [`crate::id::queue_item_prefix`]). An unknown prefix is `NotFound`;
 * an ambiguous one is refused, naming the items it matches.
 */
export interface ResolveItemCommand {
  "cmd": "resolve_item";
  "prefix": string;
}

export function resolveItem(args: Omit<ResolveItemCommand, "cmd">): ResolveItemCommand {
  return { cmd: "resolve_item", ...args };
}

export interface ResumeCommand {
  "cmd": "resume";
}

export function resume(): ResumeCommand {
  return { cmd: "resume" };
}

export interface RetryInitiativeCommand {
  "cmd": "retry_initiative";
  "initiative": InitiativeId;
}

export function retryInitiative(args: Omit<RetryInitiativeCommand, "cmd">): RetryInitiativeCommand {
  return { cmd: "retry_initiative", ...args };
}

export interface RetryItemCommand {
  "cmd": "retry_item";
  "brief_append"?: string | null;
  "fresh"?: boolean;
  "item": QueueItemId;
  "model_plan"?: ModelPlanDraft | null;
  "model_reasons"?: Record<string, string> | null;
  "stage_timeouts"?: Record<string, number>;
}

export function retryItem(args: Omit<RetryItemCommand, "cmd">): RetryItemCommand {
  return { cmd: "retry_item", ...args };
}

/**
 * A run's stages as it executed them; feature `run_plan`.
 */
export interface RunPlanCommand {
  "cmd": "run_plan";
  "run": PipelineRunId;
}

export function runPlan(args: Omit<RunPlanCommand, "cmd">): RunPlanCommand {
  return { cmd: "run_plan", ...args };
}

/**
 * Operator chat line to the orchestrator. CLI only. `text` may be
 * blank when there is at least one image.
 */
export interface SayCommand {
  "cmd": "say";
  "images"?: Array<ImageAttachment>;
  "text": string;
}

export function say(args: Omit<SayCommand, "cmd">): SayCommand {
  return { cmd: "say", ...args };
}

/**
 * Operator only; persisted in the global settings overlay.
 */
export interface SetCapsCommand {
  "cmd": "set_caps";
  "caps": Caps;
}

export function setCaps(args: Omit<SetCapsCommand, "cmd">): SetCapsCommand {
  return { cmd: "set_caps", ...args };
}

/**
 * Record a standing operator directive. It steers the orchestrator's
 * choices and never raises the ceiling, so the MCP proxy may send it.
 */
export interface SetDirectiveCommand {
  "cmd": "set_directive";
  "expires_in_hours"?: number | null;
  "text": string;
}

export function setDirective(args: Omit<SetDirectiveCommand, "cmd">): SetDirectiveCommand {
  return { cmd: "set_directive", ...args };
}

/**
 * Operator only: replace the overlay stored at one scope whole.
 */
export interface SetSettingsCommand {
  "cmd": "set_settings";
  "repo"?: RepoRef | null;
  "settings": OperatorSettings;
}

export function setSettings(args: Omit<SetSettingsCommand, "cmd">): SetSettingsCommand {
  return { cmd: "set_settings", ...args };
}

/**
 * The operator settings stored for a repository, or globally, and the
 * values in force with their sources.
 */
export interface SettingsCommand {
  "cmd": "settings";
  "repo"?: RepoRef | null;
}

export function settings(args: Omit<SettingsCommand, "cmd">): SettingsCommand {
  return { cmd: "settings", ...args };
}

/**
 * CLI only.
 */
export interface ShutdownCommand {
  "cmd": "shutdown";
}

export function shutdown(): ShutdownCommand {
  return { cmd: "shutdown" };
}

export interface StatusCommand {
  "cmd": "status";
  "waiting_limit"?: number | null;
  "waiting_offset"?: number;
}

export function status(args: Omit<StatusCommand, "cmd">): StatusCommand {
  return { cmd: "status", ...args };
}

/**
 * Change a queued or running item without cancelling it.
 */
export interface SteerItemCommand {
  "cmd": "steer_item";
  "item": QueueItemId;
  "steer": Steer;
}

export function steerItem(args: Omit<SteerItemCommand, "cmd">): SteerItemCommand {
  return { cmd: "steer_item", ...args };
}

/**
 * Stream changes with `seq > after` (`None`: live only); feature `changes`.
 */
export interface SubscribeChangesCommand {
  "cmd": "subscribe_changes";
  "after"?: number | null;
}

export function subscribeChanges(args: Omit<SubscribeChangesCommand, "cmd">): SubscribeChangesCommand {
  return { cmd: "subscribe_changes", ...args };
}

/**
 * Stream feed items with `seq > after` (`None`: live only).
 */
export interface SubscribeFeedCommand {
  "cmd": "subscribe_feed";
  "after"?: number | null;
  "verbose": boolean;
}

export function subscribeFeed(args: Omit<SubscribeFeedCommand, "cmd">): SubscribeFeedCommand {
  return { cmd: "subscribe_feed", ...args };
}

/**
 * Items and their typed edges as of one change; feature `task_graph`.
 */
export interface TaskGraphCommand {
  "cmd": "task_graph";
  "filter"?: TaskFilter;
  "limit"?: number | null;
}

export function taskGraph(args: Omit<TaskGraphCommand, "cmd">): TaskGraphCommand {
  return { cmd: "task_graph", ...args };
}

/**
 * A change `archon mcp` forwards under its own request id, recorded with
 * its outcome for `request_status` (`archon_core::tracked`).
 */
export interface TrackedCommand {
  "cmd": "tracked";
  "idempotency_key"?: string | null;
  "request": Request;
  "request_id": RequestId;
}

export function tracked(args: Omit<TrackedCommand, "cmd">): TrackedCommand {
  return { cmd: "tracked", ...args };
}

export interface UndoAuthorityCommand {
  "cmd": "undo_authority";
  "change": number;
}

export function undoAuthority(args: Omit<UndoAuthorityCommand, "cmd">): UndoAuthorityCommand {
  return { cmd: "undo_authority", ...args };
}

export interface UnlinkIssueCommand {
  "cmd": "unlink_issue";
  "item": QueueItemId;
  "ref": GithubRef;
  "relation": ManualLinkRelation;
}

export function unlinkIssue(args: Omit<UnlinkIssueCommand, "cmd">): UnlinkIssueCommand {
  return { cmd: "unlink_issue", ...args };
}

/**
 * Append to the shared contract or replace it; live members are told.
 */
export interface UpdateInitiativeCommand {
  "cmd": "update_initiative";
  "initiative": InitiativeId;
}

export function updateInitiative(args: Omit<UpdateInitiativeCommand, "cmd">): UpdateInitiativeCommand {
  return { cmd: "update_initiative", ...args };
}

export interface UpdateItemCommand {
  "cmd": "update_item";
  "item": QueueItemId;
  "patch": ItemPatch;
}

export function updateItem(args: Omit<UpdateItemCommand, "cmd">): UpdateItemCommand {
  return { cmd: "update_item", ...args };
}

/**
 * CLI only: the operator edits, pins, accepts or retracts a lesson,
 * and that always wins over what was learned.
 */
export interface UpdateLessonCommand {
  "cmd": "update_lesson";
  "lesson": LessonId;
  "patch": LessonPatch;
}

export function updateLesson(args: Omit<UpdateLessonCommand, "cmd">): UpdateLessonCommand {
  return { cmd: "update_lesson", ...args };
}

/**
 * Operator only: install, drain, then restart archond and Archon's own
 * aetherd. Accepted at once; each step is reported in the feed.
 */
export interface UpgradeCommand {
  "cmd": "upgrade";
  "aether"?: UpgradeTarget | null;
  "archon"?: UpgradeTarget | null;
  "drain_secs": number;
  "keep_aether"?: boolean;
  "requeue"?: boolean;
}

export function upgrade(args: Omit<UpgradeCommand, "cmd">): UpgradeCommand {
  return { cmd: "upgrade", ...args };
}

/** Every command the daemon accepts. */
export type Command =
  | AnswerCommand
  | AskOperatorCommand
  | AutonomyCommand
  | CancelInitiativeCommand
  | CancelItemCommand
  | ChangesPageCommand
  | CheckUpdatesCommand
  | ClearDirectiveCommand
  | ClearNotificationsCommand
  | ConfigureGithubWatcherCommand
  | ConfigureRemoteCommand
  | CostReportCommand
  | EnqueueCommand
  | EnqueueInitiativeCommand
  | FeedHistoryCommand
  | ForAgentCommand
  | GcCommand
  | GetInitiativeCommand
  | GetItemCommand
  | GetStageCommand
  | GithubReplyCommand
  | GithubWatcherCommand
  | GuideCommand
  | HelloCommand
  | ImportIssueCommand
  | IssueItemsCommand
  | ItemDiffCommand
  | ItemHistoryCommand
  | ItemLinksCommand
  | LearningCorrectCommand
  | LearningRestoreMemoryCommand
  | LearningRetryMemoryCommand
  | LearningUndoCommand
  | LearningWhyCommand
  | LessonStatsCommand
  | LessonsCommand
  | LessonsWhyCommand
  | LinkIssueCommand
  | ListArtifactsCommand
  | ListGithubObjectsCommand
  | ListInitiativesCommand
  | ListItemSummariesCommand
  | ListItemsCommand
  | ListPipelinesCommand
  | ListQuestionsCommand
  | ListReportsCommand
  | MergeItemCommand
  | MessageOrchestratorCommand
  | ModelEvidenceCommand
  | ModelPolicyCommand
  | NotifyCommand
  | OperatorCommandCommand
  | OrchestratorAttachCommand
  | OrchestratorRestartCommand
  | OrchestratorStatusCommand
  | PauseCommand
  | PromoteCommand
  | PublishArtifactCommand
  | PublishReportCommand
  | ReadArtifactCommand
  | RecordChatAnswerCommand
  | RecordLessonCommand
  | RefreshItemCommand
  | RemotePairCommand
  | RemoteRevokeCommand
  | RemoteStatusCommand
  | ReopenReportCommand
  | RepoAddCommand
  | RepoListCommand
  | RepoRemoveCommand
  | RepoUpdateCommand
  | RequestAuthorityCommand
  | RequestMergeCommand
  | RequestStatusCommand
  | ResetSettingsCommand
  | ResolveItemCommand
  | ResumeCommand
  | RetryInitiativeCommand
  | RetryItemCommand
  | RunPlanCommand
  | SayCommand
  | SetCapsCommand
  | SetDirectiveCommand
  | SetSettingsCommand
  | SettingsCommand
  | ShutdownCommand
  | StatusCommand
  | SteerItemCommand
  | SubscribeChangesCommand
  | SubscribeFeedCommand
  | TaskGraphCommand
  | TrackedCommand
  | UndoAuthorityCommand
  | UnlinkIssueCommand
  | UpdateInitiativeCommand
  | UpdateItemCommand
  | UpdateLessonCommand
  | UpgradeCommand;
