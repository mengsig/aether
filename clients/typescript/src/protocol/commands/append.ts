// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  NoteContext,
} from "../../protocol.ts";

/**
 * Append a note to an agent's transcript without starting a turn. People
 * see it at once; the model sees it only with `context: digest`, in the
 * next turn's runtime state. Needs `features::NOTES`.
 */
export interface AppendNoteCommand {
  "cmd": "append_note";
  "agent": AgentId;
  "context"?: NoteContext;
  "links"?: Array<string>;
  "text": string;
}

export function appendNote(args: Omit<AppendNoteCommand, "cmd">): AppendNoteCommand {
  return { cmd: "append_note", ...args };
}
