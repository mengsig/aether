// What a view mode draws of the transcript: each part of each row gets its row kind (the parity
// fixture's vocabulary) and that kind's outcome; a row with nothing left to draw leaves the list.

import { invariant } from "../fleet/invariant.ts";
import { outcome, type ModeRules, type Outcome, type RowKind } from "./modes.ts";
import type { ConversationRow, Part } from "./rows.ts";

export interface ViewPart {
  readonly part: Part;
  readonly kind: RowKind;
  readonly outcome: Exclude<Outcome, "hidden">;
}

export interface ViewRow {
  readonly row: ConversationRow;
  readonly parts: readonly ViewPart[];
  /** Model, cost and timing: the `usage` category. */
  readonly usage: boolean;
  /** Every part is a one-line row, so the message draws without its header. */
  readonly slim: boolean;
}

/** Harness tools whose effect is coordination; another row (a card) shows what they did. */
const COORDINATION: Readonly<Record<string, RowKind>> = { delegate: "spawned", send_message: "sent", wait_for: "waiting", ask_user: "question", remember: "memory" };
const EDITS = new Set(["edit_file", "edit", "multiedit", "notebookedit", "write_file", "write", "apply_patch", "file_change"]);

/** Lower case, without the `mcp__aether__` wrapper native engines put on harness tools. */
export function canonicalTool(name: string): string {
  const lower = name.toLowerCase();
  for (const prefix of ["mcp__aether__", "aether__"]) if (lower.startsWith(prefix)) return lower.slice(prefix.length);
  return lower;
}

function toolKind(part: Extract<Part, { kind: "tool" }>): RowKind {
  const tool = canonicalTool(part.name);
  const result = part.result;
  if (result === null) return "tool_running";
  if (!result.isError) return COORDINATION[tool] ?? "tool_succeeded";
  if (result.text.startsWith("Tool call denied")) return "tool_denied";
  if (result.text.startsWith("Tool call was cancelled")) return "tool_cancelled";
  return EDITS.has(tool) ? "edit_failed" : "tool_failed";
}

/** A part's row kind; `task` marks the row that carried the agent's task from its parent. */
export function partKind(part: Part, row: ConversationRow, task: boolean): RowKind {
  switch (part.kind) {
    case "text":
    case "attachment":
      if (row.role === "assistant") return "assistant_text";
      return task ? "user_task" : "user_operator";
    case "reasoning": return "reasoning";
    case "tool": return toolKind(part);
    case "orphan-result": return part.result.isError ? "tool_failed" : "tool_succeeded";
    case "compaction": return "compaction";
  }
}

const RANK: Readonly<Record<Outcome, number>> = { hidden: 0, line: 1, compact: 2, full: 3 };

/** The TUI draws a failed edit as two cells, its named call and the edit; one part here stands for both. */
function partOutcome(rules: ModeRules | null, kind: RowKind): Outcome {
  const drawn = outcome(rules, kind);
  if (kind !== "edit_failed") return drawn;
  const call = outcome(rules, "tool_failed");
  invariant(call !== "hidden", "every mode draws a failed call");
  return RANK[call] > RANK[drawn] ? call : drawn;
}

export interface ViewOptions {
  /** The mode's rules; null draws everything whole (the standard view, or Details). */
  readonly rules: ModeRules | null;
  /** The key of the row that carried a delegated agent's task, if it has one. */
  readonly taskRow: string | null;
}

export function viewRow(row: ConversationRow, options: ViewOptions): ViewRow | null {
  const task = row.key === options.taskRow;
  const parts: ViewPart[] = [];
  for (const part of row.parts) {
    const kind = partKind(part, row, task);
    const drawn = partOutcome(options.rules, kind);
    if (drawn !== "hidden") parts.push({ part, kind, outcome: drawn });
  }
  const usage = row.meta !== null && row.meta.error === null && outcome(options.rules, "usage") !== "hidden";
  const failed = row.meta?.error != null;
  if (parts.length === 0 && !failed && !row.live) return null;
  invariant(parts.length <= row.parts.length, "a mode only removes parts");
  const slim = parts.length > 0 && !failed && parts.every((p) => p.outcome !== "full");
  return { row, parts, usage, slim };
}

/** The turn aether-context writes when a new transcript starts from its task (`ensure_ends_with_user`). */
export const TASK_TURN = "Your task:\n";

/**
 * A delegated agent's task row: its first user turn, when that turn is the task as aether-context
 * wrote it. Any other turn, an inbox delivery included, may be a person's text and is drawn as theirs.
 */
export function taskRowKey(rows: readonly ConversationRow[], delegated: boolean): string | null {
  if (!delegated) return null;
  const first = rows.find((row) => row.role === "user");
  const text = first?.parts[0];
  // A delivery's header moved into the row's head (speaker.from); its body is a person's text, not the task.
  if (first === undefined || first.speaker.from !== null) return null;
  if (text?.kind !== "text" || !text.text.startsWith(TASK_TURN)) return null;
  invariant(first.seq !== null, "a task turn is a committed entry");
  return first.key;
}

/** The rows a mode draws, in order; cached per row so appending one recomputes one. */
export function viewRows(rows: readonly ConversationRow[], options: ViewOptions, cache: WeakMap<ConversationRow, ViewRow | null>): ViewRow[] {
  const out: ViewRow[] = [];
  for (const row of rows) {
    let view = cache.get(row);
    if (view === undefined) {
      view = viewRow(row, options);
      cache.set(row, view);
    }
    if (view !== null) out.push(view);
  }
  invariant(out.length <= rows.length, "a mode never adds rows");
  return out;
}
