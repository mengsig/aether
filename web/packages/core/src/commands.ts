// What each daemon command needs from its connection, and its idempotency key.
// The Aether table mirrors crates/aether-protocol access.rs, checked by a test.

import type { Standing } from "@aether-ui/protocol";
import { RefusedError } from "./errors.ts";
import { string, type Json } from "./guards.ts";

export type Access = "observe" | "operate" | "host_only" | "device_self";

const words = (text: string): ReadonlySet<string> => new Set(text.split(/\s+/).filter(Boolean));

export const AETHER_OBSERVE = words(`hello status preview_retention storage_status ping get_agent list_agents get_tree
  get_github_watcher get_schedule list_schedules schedule_history get_task list_tasks list_approvals list_approval_rules
  subscribe unsubscribe get_events get_transcript get_inbox get_snapshot get_invocation list_invocations get_checkpoint
  latest_checkpoint get_artifact list_artifacts read_artifact memories list_runs get_run_report replay list_models
  auth_status providers get_attention list_workspaces get_push_config list_repository_aliases list_notifications
  get_notification mcp_servers list_agents_page get_transcript_page usage_breakdown noesis_status
  noesis_get_session noesis_list_sessions noesis_graph noesis_jobs noesis_learner noesis_learner_evidence
  memory_migration_status`);

export const AETHER_OPERATE = words(`create_agent reply_to_question send_message append_note cancel interrupt
  configure_github_watcher create_schedule update_schedule trigger restart wake pause resume archive set_approvals
  compact create_task revoke_approval_rule publish_artifact create_memory forget_memory restore_memory pin_memory
  sync_repository_knowledge complete_path validate_workspace get_git_summary workspace_diff workspace_integrate
  workspace_discard refresh_provider_usage post_notification mark_notification_read resolve_notification
  answer_notification list_devices update_agent resolve_approval add_approval_rule resolve_submission
  noesis_configure noesis_create_session noesis_send noesis_answer noesis_delete_session noesis_delete_proposition
  noesis_delete_source noesis_extract noesis_import_source noesis_cancel_job noesis_correct_learner`);

/** Host-only and remote-admin alike: only a `local` connection may send them. */
export const AETHER_HOST_ONLY = words(`storage_gc shutdown engine_tool engine_tools export_run import_run alias_repository
  accept_memory reject_memory memories_in_context record_memory_feedback retract_memory_feedback memory_feedback
  get_remote configure_remote start_pairing get_pairing confirm_pairing cancel_pairing revoke_device update_device
  memory_retrieval_explain consolidate_memories memory_export memory_import`);

export const AETHER_DEVICE_SELF = words("subscribe_push unsubscribe_push test_push");

/** Aether commands that accept an `idempotency_key` (generated from the schema; checked by a test). */
export const AETHER_KEYED = words(`add_approval_rule archive cancel create_agent create_memory create_schedule create_task
  interrupt pause record_memory_feedback reply_to_question resolve_approval restart resume send_message set_approvals
  trigger update_agent update_schedule noesis_answer noesis_cancel_job noesis_configure noesis_correct_learner
  noesis_create_session noesis_delete_proposition noesis_delete_session noesis_delete_source noesis_extract
  noesis_import_source noesis_send`);

/** Archon `Read` requests (archon-core Request::class) and the protocol 24 read models. */
export const ARCHON_READ = words(`hello request_status status guide repo_list list_items list_item_summaries get_item
  get_stage resolve_item list_pipelines model_policy list_questions subscribe_feed feed_history list_reports
  orchestrator_status cost_report model_evidence autonomy settings github_watcher list_initiatives get_initiative lessons
  lessons_why lesson_stats learning_why list_artifacts item_diff read_artifact subscribe_changes changes_page
  item_history task_graph run_plan item_links issue_items list_github_objects`);

/** Archon OperatorRead and OperatorChange requests a device may not send (archon-remote allow.rs). */
// `answer` and `say` are OperatorChange, yet operator devices may send them.
export const ARCHON_HOST_ONLY = words(`check_updates remote_status operator_command undo_authority shutdown repo_remove
  repo_update set_caps set_settings reset_settings orchestrator_restart orchestrator_attach upgrade gc
  configure_github_watcher update_lesson learning_correct learning_undo learning_restore_memory learning_retry_memory
  merge_item configure_remote remote_pair remote_revoke`);

export function aetherAccess(command: Json): Access {
  const cmd = string(command.cmd, "command.cmd");
  const special = fieldAccess(cmd, command);
  if (special) return special;
  if (AETHER_OBSERVE.has(cmd)) return "observe";
  if (AETHER_DEVICE_SELF.has(cmd)) return "device_self";
  if (AETHER_HOST_ONLY.has(cmd)) return "host_only";
  // Unknown commands are treated as changes: the conservative side of the check.
  return "operate";
}

/** The guarded arms of access.rs, where a field widens what a command reaches. */
function fieldAccess(cmd: string, command: Json): Access | null {
  const field = (name: string): Json => objectAt(command, name);
  if (cmd === "create_memory" && field("spec").proposed === true) return "host_only";
  if (cmd === "memories" && field("query").proposed === true) return "host_only";
  if (cmd === "list_devices") return command.include_revoked === true ? "host_only" : "operate";
  if (cmd === "mcp_servers") return command.refresh === true ? "operate" : "observe";
  if (cmd === "update_agent" && (field("patch").backend != null || field("patch").tools != null)) return "host_only";
  if (isGlobalRule(command)) return "host_only";
  if (cmd === "noesis_configure") return noesisConfigureAccess(field("patch"));
  if (cmd === "resolve_submission") {
    // Asking what became of a command needs the standing to send it.
    const inner = typeof field("command").cmd === "string" ? aetherAccess(field("command")) : "operate";
    return inner === "observe" ? "operate" : inner;
  }
  return null;
}

/** The object under `name`, or an empty one when it is absent or not an object. */
function objectAt(json: Json, name: string): Json {
  const value = json[name];
  return value !== null && typeof value === "object" && !Array.isArray(value) ? (value as Json) : {};
}

/** Mirrors noesis::configure_access: enabled, budgets, billing and prompt overrides are the host's. */
function noesisConfigureAccess(patch: Json): Access {
  const part = (name: string): Json => objectAt(patch, name);
  const budget = part("budget");
  const setsBudget = ["interactive_usd_daily", "background_usd_daily", "background_tokens_daily"]
    .some((key) => budget[key] != null);
  const reachesHost = patch.enabled != null || setsBudget || part("billing").allow_api != null
    || part("prompts").overrides != null;
  return reachesHost ? "host_only" : "operate";
}

function isGlobalRule(command: Json): boolean {
  const scope = command.cmd === "add_approval_rule" ? command.scope
    : command.cmd === "resolve_approval" ? ((command.remember ?? {}) as Json).scope : undefined;
  return scope !== null && typeof scope === "object" && (scope as Json).scope === "everywhere";
}

export function archonAccess(command: Json): Access {
  const cmd = string(command.cmd, "command.cmd");
  if (cmd === "pause" || cmd === "resume") {
    const target = command as Json;
    if (target.scope === "global" && target.hard === true) return "host_only";
  }
  if (ARCHON_READ.has(cmd)) return "observe";
  if (ARCHON_HOST_ONLY.has(cmd)) return "host_only";
  return "operate";
}

/** Throws a RefusedError, with the reason a disabled control shows, when the standing does not allow it. */
export function checkAllowed(access: Access, standing: Standing | undefined): void {
  if (standing === undefined) throw new RefusedError("The connection is not ready");
  if (access === "host_only" && standing !== "local") throw new RefusedError("Only the daemon's own host can do this");
  if (access === "operate" && standing === "observer") throw new RefusedError("Observer devices cannot change daemon state");
}

/** Whether core keys this Aether command, so a lost reply is tracked for the recovery review. */
export function isKeyed(command: Json): boolean {
  return AETHER_KEYED.has(string(command.cmd, "command.cmd"));
}

/** Adds an idempotency key to a keyed Aether command that has none, so a resend cannot act twice. */
export function withIdempotencyKey(command: Json, makeKey: () => string = () => crypto.randomUUID()): Json {
  if (!AETHER_KEYED.has(string(command.cmd, "command.cmd")) || typeof command.idempotency_key === "string") return command;
  const key = makeKey();
  string(key, "idempotency key");
  return { ...command, idempotency_key: key };
}
