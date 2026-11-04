// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  NoesisProvenance,
} from "../../protocol.ts";

export interface Proposition {
  "aliases": Array<string>;
  "created_at": string;
  "id": string;
  "name": string;
  "provenance": Array<NoesisProvenance>;
  "statement": string;
}
