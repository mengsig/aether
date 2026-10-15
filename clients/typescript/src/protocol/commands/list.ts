// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  AgentQuery,
  ArtifactQuery,
  TaskQuery,
} from "../../protocol.ts";


export interface ListAgentsCommand {
  "cmd": "list_agents";
  "query": AgentQuery;
}

export function listAgents(args: Omit<ListAgentsCommand, "cmd">): ListAgentsCommand {
  return { cmd: "list_agents", ...args };
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
 * Workspaces holding root agents, most recently active first.
 */
export interface ListWorkspacesCommand {
  "cmd": "list_workspaces";
  "limit"?: number | null;
}

export function listWorkspaces(args: Omit<ListWorkspacesCommand, "cmd">): ListWorkspacesCommand {
  return { cmd: "list_workspaces", ...args };
}
