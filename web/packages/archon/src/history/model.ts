// The history explorer's derivations: a search from what the form holds, and an item's timeline from
// the `item_history` transitions core stored. Pure, so the tests check them without a screen.

import type { ItemSearch } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import type { Status } from "@aether-ui/ui";
import { invariant, isStateKind, statePill } from "../queue/model.ts";

export type Transition = archon.ItemTransition;

const DAY_MS = 86_400_000;
const TEXT_MAX = 200;
export const SEARCH_LIMIT = 200;

export interface SearchForm {
  readonly text: string;
  /** `YYYY-MM-DD` days in UTC, as a date input holds them; empty is open-ended. */
  readonly from: string;
  readonly to: string;
}

/** The last seven days, ending today. */
export function defaultForm(now: number): SearchForm {
  invariant(Number.isFinite(now), "the default range ends at a time");
  return { text: "", from: new Date(now - 6 * DAY_MS).toISOString().slice(0, 10), to: "" };
}

const dayStart = (day: string): number => Date.parse(`${day}T00:00:00Z`);

/** The search the form asks for, or why it cannot be sent. `to` includes its whole day. */
export function searchOf(form: SearchForm): { query: ItemSearch } | { error: string } {
  const text = form.text.trim();
  if ([...text].length > TEXT_MAX) return { error: `Search for at most ${TEXT_MAX} characters.` };
  const since = form.from === "" ? null : dayStart(form.from);
  const before = form.to === "" ? null : dayStart(form.to) + DAY_MS;
  if (Number.isNaN(since) || Number.isNaN(before)) return { error: "Pick the days from the calendar." };
  if (since !== null && before !== null && since >= before) return { error: "The range starts after it ends." };
  const query: ItemSearch = {
    limit: SEARCH_LIMIT,
    ...(text === "" ? {} : { text }),
    ...(since === null ? {} : { updated_since: new Date(since).toISOString() }),
    ...(before === null ? {} : { before: new Date(before).toISOString() }),
  };
  return { query };
}

/** The search of a form known to be valid, such as the default one. */
export function queryOf(form: SearchForm): ItemSearch {
  const result = searchOf(form);
  invariant("query" in result, `the form is a valid search: ${"error" in result ? result.error : ""}`);
  return result.query;
}

export interface TimelineRow {
  readonly seq: number;
  readonly at: string;
  readonly from: { status: Status; label: string } | null;
  readonly to: { status: Status; label: string };
  readonly reason: string | null;
  readonly by: string;
  /** How long the item stayed in `to`: until the next transition, or until `now` for the last. */
  readonly heldMs: number;
}

const ACTORS: Readonly<Record<string, string>> = { daemon: "Archon", cli: "CLI", web: "Web", mcp: "MCP", github: "GitHub" };
export const actorLabel = (by: string): string => ACTORS[by] ?? by;

function pillOf(kind: string): { status: Status; label: string } {
  invariant(isStateKind(kind), `a transition names a known state, not ${kind}`);
  return statePill(kind, false);
}

/** The transitions, oldest first, as `item_history` gives them, each with how long it held. */
export function timeline(transitions: readonly Transition[], now: number): TimelineRow[] {
  invariant(Number.isFinite(now), "a timeline ends at a time");
  return transitions.map((row, index) => {
    const next = transitions[index + 1];
    invariant(next === undefined || next.seq > row.seq, "transitions are in ascending seq order");
    const start = Date.parse(row.at);
    invariant(Number.isFinite(start), `transition ${row.seq} has a time`);
    const end = next === undefined ? now : Date.parse(next.at);
    return {
      seq: row.seq, at: row.at, from: row.from ? pillOf(row.from) : null, to: pillOf(row.to),
      reason: row.reason ?? null, by: actorLabel(row.by), heldMs: Math.max(0, end - start),
    };
  });
}

/** A compact duration: seconds, minutes, hours and minutes, or days and hours. */
export function duration(ms: number): string {
  invariant(ms >= 0 && Number.isFinite(ms), "a duration is a finite non-negative span");
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 === 0 ? `${hours} h` : `${hours} h ${minutes % 60} min`;
  const days = Math.floor(hours / 24);
  return hours % 24 === 0 ? `${days} d` : `${days} d ${hours % 24} h`;
}
