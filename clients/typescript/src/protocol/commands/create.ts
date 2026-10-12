// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentConfig,
  AgentId,
  ApprovalPolicy,
  CreateScheduleSpec,
  ModelRef,
  OperatorMemorySpec,
  Priority,
  RunId,
  Scope,
  TaskId,
} from "../../protocol.ts";


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
