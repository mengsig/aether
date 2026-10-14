// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
} from "../../protocol.ts";

/**
 * Operator applicability, not authorship. Repository identity is resolved by the runtime.
 * Tagged by `scope`.
 */
export interface OperatorMemoryScopeAgent {
  "id": AgentId;
  "scope": "agent";
}

export interface OperatorMemoryScopeGlobal {
  "scope": "global";
}

export interface OperatorMemoryScopeRepository {
  "root"?: string | null;
  "scope": "repository";
}

export interface OperatorMemoryScopeRole {
  "role": string;
  "scope": "role";
}

export interface OperatorMemoryScopeRepositoryRole {
  "role": string;
  "root"?: string | null;
  "scope": "repository_role";
}

export type OperatorMemoryScope =
    OperatorMemoryScopeAgent |
    OperatorMemoryScopeGlobal |
    OperatorMemoryScopeRepository |
    OperatorMemoryScopeRole |
    OperatorMemoryScopeRepositoryRole
;
