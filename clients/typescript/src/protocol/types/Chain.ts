// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  NoesisProvenance,
} from "../../protocol.ts";

export interface Chain {
  "conclusion": string;
  "created_at": string;
  "id": string;
  "premises": Array<string>;
  "provenance": Array<NoesisProvenance>;
}
