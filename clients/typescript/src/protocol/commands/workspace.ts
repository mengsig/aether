// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
} from "../../protocol.ts";

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
