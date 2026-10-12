// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ApprovalRuleId,
  DeviceId,
} from "../../protocol.ts";


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
