// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  PathCompletion,
} from "../../protocol.ts";

export interface PathCompletions {
  "entries": Array<PathCompletion>;
  "truncated": boolean;
}
