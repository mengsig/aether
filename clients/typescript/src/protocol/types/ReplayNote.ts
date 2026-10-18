// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  NoteContext,
  NoteLevel,
} from "../../protocol.ts";

/**
 * A note appended to the transcript; it started no turn.
 */
export interface ReplayNote {
  "at": string;
  "context"?: NoteContext;
  "level"?: NoteLevel;
  "links"?: Array<string>;
  "seq": number;
  "text": string;
}
