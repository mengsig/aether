// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ConversationEntryKind,
} from "../../protocol.ts";

export interface ConversationEntry {
  "at": string;
  "important": boolean;
  "kind": ConversationEntryKind;
  "links"?: Array<string>;
  "part": number;
  "seq": number;
  "text": string;
}
