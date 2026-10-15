// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  MemoryId,
  MemoryKind,
  MemoryRecorder,
  MemoryScope,
} from "../../protocol.ts";

export interface MemoryQuerySpec {
  "id"?: MemoryId | null;
  "include_forgotten"?: boolean;
  "include_superseded"?: boolean;
  "kinds"?: Array<MemoryKind>;
  "limit"?: number;
  "proposed"?: boolean;
  "provenance_agent"?: AgentId | null;
  "recorded_by"?: MemoryRecorder | null;
  "scope"?: MemoryScope | null;
  "tags"?: Array<string>;
  "text"?: string | null;
}
