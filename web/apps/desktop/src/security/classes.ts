// The local bridge passes observe/operate commands and explicitly listed request-only host reads.
// Other host commands need privileged actions; shutdown never passes.

import assert from "node:assert/strict";

export type DaemonKind = "aether" | "archon";

/** Aether's `Access` classes (crates/aether-protocol/src/access.rs), command by command. */
export const AETHER_OBSERVE: ReadonlySet<string> = new Set([
  "auth_status", "get_agent", "get_artifact", "get_attention", "get_checkpoint", "get_events",
  "get_github_watcher", "get_inbox", "get_invocation", "get_notification", "get_push_config",
  "get_run_report", "get_schedule", "get_snapshot", "get_task", "get_transcript", "get_transcript_page",
  "get_tree", "hello", "latest_checkpoint", "list_agents", "list_agents_page", "list_approval_rules",
  "list_approvals", "list_artifacts", "list_invocations", "list_models", "list_view_modes", "list_notifications",
  "list_repository_aliases", "list_runs", "list_schedules", "list_tasks", "list_workspaces", "memories",
  "memory_migration_status",
  "noesis_get_session", "noesis_graph", "noesis_jobs", "noesis_learner", "noesis_learner_evidence",
  "noesis_list_sessions", "noesis_status", "ping", "preview_retention", "providers", "read_artifact",
  "replay", "schedule_history", "status", "storage_status", "subscribe", "unsubscribe", "usage_breakdown",
]);

export const AETHER_OPERATE: ReadonlySet<string> = new Set([
  "answer_notification", "append_note", "archive", "cancel", "compact", "complete_path",
  "configure_github_watcher", "create_agent", "create_memory", "create_schedule", "create_task",
  "forget_memory", "get_git_summary", "interrupt", "mark_notification_read", "noesis_answer",
  "noesis_cancel_job", "noesis_correct_learner", "noesis_create_session", "noesis_delete_proposition",
  "noesis_delete_session", "noesis_delete_source", "noesis_extract", "noesis_import_source", "noesis_send",
  "pause", "pin_memory", "post_notification", "publish_artifact", "refresh_provider_usage",
  "reply_to_question", "resolve_notification", "restart", "restore_memory", "resume", "revoke_approval_rule",
  "send_message", "set_approvals", "sync_repository_knowledge", "trigger", "update_schedule",
  "validate_workspace", "wake", "workspace_diff", "workspace_discard", "workspace_integrate",
]);

export const AETHER_HOST_ONLY: ReadonlySet<string> = new Set([
  "accept_memory", "alias_repository", "engine_tool", "engine_tools", "export_run", "import_run",
  "consolidate_memories", "memories_in_context", "memory_export", "memory_feedback", "memory_import",
  "memory_retrieval_explain", "record_memory_feedback", "reject_memory", "retract_memory_feedback", "shutdown",
  "storage_gc",
]);

export const AETHER_REMOTE_ADMIN: ReadonlySet<string> = new Set([
  "cancel_pairing", "configure_remote", "confirm_pairing", "get_pairing", "get_remote", "revoke_device",
  "start_pairing", "update_device",
]);

/** Host status reads allowed only as requests on the local Aether connection. */
export const AETHER_HOST_READS: ReadonlySet<string> = new Set(["get_remote"]);

/** A paired device acting on its own record: never the desktop's to send. */
export const AETHER_DEVICE_SELF: ReadonlySet<string> = new Set(["subscribe_push", "test_push", "unsubscribe_push"]);

/** Commands whose class a field decides; `aetherFieldClass` mirrors each rule. */
export const AETHER_BY_FIELD: ReadonlySet<string> = new Set([
  "add_approval_rule", "create_memory", "list_devices", "mcp_servers", "memories", "noesis_configure",
  "resolve_approval", "resolve_submission", "update_agent",
]);

/**
 * What Archon lets an operator device send (archon-remote `allow.rs`, Archon 9a8d63d1), and
 * `get_initiative`, a `Read`. The app's connections greet as the web app, so archond marks its
 * `say` and `answer` as a device's and refuses it authority confirmations: no CLI standing.
 * The web packages' `archonAccess` must agree, which test/security/ui-parity.test.ts checks.
 * That standing exists from Archon 1.5.0 (protocol 24) on: `ARCHON_NEEDS_STANDING` commands pass
 * only on a connection whose welcome reported at least that protocol.
 */
export const ARCHON_PASS: ReadonlySet<string> = new Set([
  "hello", "status", "repo_list", "list_items", "list_item_summaries", "get_item", "get_stage",
  "resolve_item", "list_pipelines", "list_questions", "feed_history", "subscribe_feed", "list_reports",
  "orchestrator_status", "cost_report", "list_artifacts", "read_artifact", "item_diff", "settings",
  "autonomy", "subscribe_changes", "changes_page", "item_history", "task_graph", "run_plan", "item_links",
  "issue_items", "list_github_objects", "get_initiative", "enqueue", "cancel_item", "retry_item", "pause",
  "resume", "answer", "say", "link_issue", "unlink_issue", "import_issue", "model_policy", "steer_item", "update_item",
]);

/** First Archon protocol (Archon 1.5.0) whose archon-web connections have a device's standing. */
export const ARCHON_STANDING_PROTOCOL = 24;
export const ARCHON_NEEDS_STANDING: ReadonlySet<string> = new Set(["say", "answer", "steer_item", "update_item"]);

/** The only client the app window's own connections may declare to archond. */
export const ARCHON_APP_CLIENT = "archon-web";

type Frame = Readonly<Record<string, unknown>>;
type AetherClass = "observe" | "operate" | "host_only" | "remote_admin" | "device_self";

function isSet(value: unknown): boolean {
  return value !== undefined && value !== null;
}

function field(frame: Frame, name: string): Frame {
  const value = frame[name];
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Frame) : {};
}

/** A remembered rule for every agent (`RuleScope` `everywhere`) is the host's own policy. */
function ruleClass(scope: Frame): AetherClass {
  return scope.scope === "everywhere" ? "host_only" : "operate";
}

function noesisConfigureClass(patch: Frame): AetherClass {
  const budget = field(patch, "budget");
  const setsBudget = ["interactive_usd_daily", "background_usd_daily", "background_tokens_daily"]
    .some((key) => isSet(budget[key]));
  const hostOnly = isSet(patch.enabled) || setsBudget || isSet(field(patch, "billing").allow_api)
    || isSet(field(patch, "prompts").overrides);
  return hostOnly ? "host_only" : "operate";
}

/** The field rules of `Command::access`, one per command of AETHER_BY_FIELD. */
function aetherFieldClass(cmd: string, frame: Frame, depth: number): AetherClass | undefined {
  switch (cmd) {
    case "create_memory": return field(frame, "spec").proposed === true ? "host_only" : "operate";
    case "memories": return field(frame, "query").proposed === true ? "host_only" : "observe";
    case "mcp_servers": return frame.refresh === true ? "operate" : "observe";
    case "list_devices": return frame.include_revoked === true ? "remote_admin" : "operate";
    case "update_agent": {
      const patch = field(frame, "patch");
      return isSet(patch.backend) || isSet(patch.tools) ? "host_only" : "operate";
    }
    case "resolve_approval": return ruleClass(field(field(frame, "remember"), "scope"));
    case "add_approval_rule": return ruleClass(field(frame, "scope"));
    case "noesis_configure": return noesisConfigureClass(field(frame, "patch"));
    case "resolve_submission": {
      if (depth > 0) return "host_only";
      const inner = aetherClass(field(frame, "command"), depth + 1);
      return inner === "observe" ? "operate" : inner;
    }
    default: throw new Error(`${cmd} has no field rule`);
  }
}

export function aetherClass(frame: Frame, depth = 0): AetherClass | undefined {
  const cmd = frame.cmd;
  if (typeof cmd !== "string") return undefined;
  if (AETHER_BY_FIELD.has(cmd)) return aetherFieldClass(cmd, frame, depth);
  if (AETHER_OBSERVE.has(cmd)) return "observe";
  if (AETHER_OPERATE.has(cmd)) return "operate";
  if (AETHER_HOST_ONLY.has(cmd)) return "host_only";
  if (AETHER_REMOTE_ADMIN.has(cmd)) return "remote_admin";
  if (AETHER_DEVICE_SELF.has(cmd)) return "device_self";
  return undefined;
}

/** Why a local frame may not pass through request/subscribe, or undefined when it may. */
export function passThroughRefusal(kind: DaemonKind, frame: Frame, archonProtocol?: number, method: "request" | "subscribe" = "request"): string | undefined {
  const cmd = frame.cmd;
  if (typeof cmd !== "string") return "A frame names its command in cmd";
  if (cmd === "shutdown") return "shutdown is never sent from the desktop app";
  if (kind === "archon") return archonRefusal(cmd, frame, archonProtocol);
  if (AETHER_HOST_READS.has(cmd)) return hostReadRefusal(frame, method);
  const access = aetherClass(frame);
  if (access === undefined) return `Unknown command ${cmd}`;
  if (access !== "observe" && access !== "operate") return `${cmd} is ${access} and needs a privileged action`;
  // The local socket skips the guards aetherd puts on a device; who the watcher obeys is one.
  const run = commandRun(frame);
  if (run.cmd === "configure_github_watcher") {
    return "configure_github_watcher sets who the watcher obeys on GitHub; change it at the host";
  }
  // The schema does not check a replayed command, and main must look its rule up.
  if (run.cmd === "revoke_approval_rule" && (typeof run.id !== "string" || run.id === "")) {
    return "revoke_approval_rule names its rule in id";
  }
  return undefined;
}

function hostReadRefusal(frame: Frame, method: "request" | "subscribe"): string | undefined {
  assert.equal(frame.cmd, "get_remote", "every host read needs an explicit argument check");
  assert.equal(aetherClass(frame), "remote_admin", "the host read retains its daemon access class");
  if (method !== "request") return "get_remote is a host read allowed only through request";
  if (!Number.isSafeInteger(frame.req) || (frame.req as number) < 1) return "get_remote requires a positive integer req";
  if (Object.keys(frame).length !== 2 || !Object.hasOwn(frame, "cmd") || !Object.hasOwn(frame, "req")) {
    return "get_remote accepts only cmd and req";
  }
  return undefined;
}

/** The command a frame runs: its own, or the one a `resolve_submission` may replay. */
function commandRun(frame: Frame): Frame {
  return frame.cmd === "resolve_submission" ? field(frame, "command") : frame;
}

/**
 * The rule an Aether frame revokes, for the bridge to look up: aetherd refuses a device the
 * revocation of a rule for every agent, and the local socket does not, so main refuses it.
 */
export function revokedRule(kind: DaemonKind, frame: Frame): string | undefined {
  if (kind !== "aether") return undefined;
  const run = commandRun(frame);
  if (run.cmd !== "revoke_approval_rule") return undefined;
  if (typeof run.id !== "string" || run.id === "") throw new Error("the schema requires a revoked rule's id");
  return run.id;
}

function archonRefusal(cmd: string, frame: Frame, protocol: number | undefined): string | undefined {
  if (!ARCHON_PASS.has(cmd)) return `${cmd} is not open to the desktop app's requests`;
  if (ARCHON_NEEDS_STANDING.has(cmd) && (protocol === undefined || protocol < ARCHON_STANDING_PROTOCOL)) {
    return `This action needs Archon 1.5.0 or newer; this archond reports protocol ${String(protocol ?? "unknown")}`;
  }
  // archond takes a connection's standing from the client its hello declares, so the app's own
  // connections greet only as the web app; CLI standing stays with main's confirmed actions.
  if (cmd === "hello" && frame.client !== ARCHON_APP_CLIENT) {
    return `the desktop app greets archond as ${ARCHON_APP_CLIENT}, not ${String(frame.client).slice(0, 32)}`;
  }
  // Who sends is the hello's to say: no other frame names a client, and Aether alone names a caller.
  if (cmd !== "hello" && "client" in frame) return `${cmd} names no client; the connection's hello did`;
  if ("caller" in frame) return `${cmd} names no caller; only Aether names an agent caller`;
  if ((cmd === "pause" || cmd === "resume") && frame.hard === true) return `${cmd} with hard is for the archon CLI`;
  // `chat` records an answer as given in archon chat; the app's answers are its own.
  if (cmd === "answer" && frame.chat === true) return "answer with chat is for archon chat; the app answers as itself";
  return undefined;
}
