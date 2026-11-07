// The TUI's view-mode rules (crates/aether-tui/src/view_modes.rs) over the shared row vocabulary of
// crates/aether-tui/tests/fixtures/view_mode_parity.json, which pins both sides to one outcome per row.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";

/** Every kind of row a transcript draws, named as the parity fixture names them. */
export const ROW_KINDS = [
  "header", "user_operator", "user_task", "user_message", "assistant_text", "reasoning",
  "tool_succeeded", "tool_running", "tool_awaiting", "tool_failed", "tool_denied", "tool_cancelled", "edit_failed",
  "memory", "spawned", "waiting", "child_finished", "child_failed", "sent", "approval", "question",
  "error", "notice_error", "notice_status", "compaction", "compaction_failed", "usage",
] as const;
export type RowKind = (typeof ROW_KINDS)[number];

/** How a mode draws a row: whole, as one compact line, as a failed call's one line, or not at all. */
export type Outcome = "full" | "compact" | "line" | "hidden";

export type ModeRules = Pick<aether.ViewMode, "show" | "density">;

const CATEGORY: Readonly<Record<RowKind, aether.ViewModeCategory>> = {
  header: "status", user_operator: "user_text", user_task: "delegation", user_message: "agent_message",
  assistant_text: "assistant_text", reasoning: "reasoning",
  tool_succeeded: "tool", tool_running: "tool", tool_awaiting: "tool", tool_failed: "tool", tool_denied: "tool",
  tool_cancelled: "tool", edit_failed: "tool", memory: "memory", spawned: "delegation", waiting: "status",
  child_finished: "completion", child_failed: "completion", sent: "agent_message", approval: "approval",
  question: "question", error: "error", notice_error: "error", notice_status: "status",
  compaction: "compaction", compaction_failed: "compaction", usage: "usage",
};

/** Rows every mode draws besides decisions and errors: the operator's text, a call stuck or refused, a failure. */
const ALWAYS: ReadonlySet<RowKind> = new Set<RowKind>([
  "header", "user_operator", "tool_awaiting", "tool_failed", "tool_denied", "tool_cancelled", "edit_failed", "child_failed", "compaction_failed",
]);
const DECISIONS: ReadonlySet<aether.ViewModeCategory> = new Set<aether.ViewModeCategory>(["question", "approval", "error"]);
const COORDINATION: ReadonlySet<aether.ViewModeCategory> = new Set<aether.ViewModeCategory>(["delegation", "agent_message", "completion", "status"]);

export const categoryOf = (kind: RowKind): aether.ViewModeCategory => CATEGORY[kind];

export function mandatory(kind: RowKind): boolean {
  return DECISIONS.has(CATEGORY[kind]) || ALWAYS.has(kind);
}

/** What `rules` (null for the standard view) draws of a `kind` row. */
export function outcome(rules: ModeRules | null, kind: RowKind): Outcome {
  invariant(kind in CATEGORY, `${kind} is a known row kind`);
  if (rules === null) return "full";
  const category = CATEGORY[kind];
  // A failed edit's line is its call's; the edit itself is tool detail.
  const shown = rules.show.includes(category) || (mandatory(kind) && kind !== "edit_failed");
  if (!shown) return "hidden";
  if (kind === "tool_failed" && !rules.show.includes("tool")) return "line";
  if (rules.density === "compact" && !mandatory(kind) && COORDINATION.has(category)) return "compact";
  return "full";
}

/** Whether a mode's scope admits this agent: an exact match on each field it names. */
export function applies(mode: Pick<aether.ViewMode, "scope">, agent: Pick<aether.Agent, "name" | "role">): boolean {
  const scope = mode.scope;
  if (scope == null) return true;
  invariant(scope.name != null || scope.role != null, "a mode's scope names a name or a role");
  return (scope.name == null || scope.name === agent.name) && (scope.role == null || scope.role === agent.role);
}

/** The user mode Archon installs for its orchestrator; it wins over any other mode scoped to the agent. */
export const CALM = "calm";

/** The mode an agent opens in: a still-valid saved choice, else Calm or another user mode scoped to it, else standard. */
export function initialMode(modes: readonly aether.ViewMode[], agent: Pick<aether.Agent, "name" | "role">, saved: string | null): string {
  invariant(modes.some((m) => m.name === "standard"), "the catalog always has the standard mode");
  const usable = modes.filter((m) => applies(m, agent));
  if (saved !== null && usable.some((m) => m.name === saved)) return saved;
  const scoped = usable.filter((m) => !m.builtin && m.scope != null);
  invariant(scoped.every((m) => m.name !== "standard"), "a user mode never replaces standard");
  return (scoped.find((m) => m.name === CALM) ?? scoped[0])?.name ?? "standard";
}

export const modeLabel = (mode: Pick<aether.ViewMode, "name" | "label">): string =>
  mode.label ?? mode.name.charAt(0).toUpperCase() + mode.name.slice(1);
