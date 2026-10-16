// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ApprovalDefault,
  LowDisk,
  McpServerHealth,
  ModelRef,
  NetworkPolicy,
  ProviderHealth,
  ResourceHolding,
  RunSummary,
  Usage,
  providers,
} from "../../protocol.ts";

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
  "providers"?: Array<ProviderHealth>;
  "resource_leases"?: Array<ResourceHolding>;
  "run_budget_usd"?: number | null;
  "runs"?: Array<RunSummary>;
  "started_at": string;
  "tasks_active": number;
  "usage_total": Usage;
  "version": string;
  "yolo"?: boolean;
}
