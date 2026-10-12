// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  AgentPatch,
  DeviceId,
  DeviceRole,
  ScheduleAction,
  ScheduleId,
} from "../../protocol.ts";


export interface UpdateAgentCommand {
  "cmd": "update_agent";
  "id": AgentId;
  "idempotency_key"?: string | null;
  "patch": AgentPatch;
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
