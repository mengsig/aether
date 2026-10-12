// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DeviceIdentity,
} from "../../protocol.ts";

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
