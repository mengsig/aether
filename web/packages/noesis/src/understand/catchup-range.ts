// Where a catch-up starts: a UTC calendar day, so the same choice made twice in one day names the same
// range (and the same earlier session). The client cannot read releases or tags, so there is no release preset.

import { invariant } from "../learn/model.ts";

export const RANGE_PRESETS = [
  { id: "visit", label: "Since my last visit" },
  { id: "week", label: "Last 7 days" },
  { id: "month", label: "Last 30 days" },
] as const;
export type RangePresetId = (typeof RANGE_PRESETS)[number]["id"];

const DAY_MS = 86_400_000;
const PRESET_DAYS: Readonly<Record<Exclude<RangePresetId, "visit">, number>> = { week: 7, month: 30 };
/** A first catch-up, with no earlier visit to start from, looks back this many days. */
export const FIRST_VISIT_DAYS = 7;

const DAY_TEXT = /^\d{4}-\d{2}-\d{2}$/;

/** Whether `text` is a real `YYYY-MM-DD` date, not only shaped like one. */
export function isSince(text: string): boolean {
  if (!DAY_TEXT.test(text)) return false;
  const at = Date.parse(`${text}T00:00:00Z`);
  return Number.isFinite(at) && new Date(at).toISOString().startsWith(text);
}

function dayOf(ms: number): string {
  invariant(Number.isFinite(ms), "a day is read from a real instant");
  return new Date(ms).toISOString().slice(0, 10);
}

/** The first moment of a `since` day, in epoch milliseconds. */
export function sinceInstant(since: string): number {
  invariant(isSince(since), `${since} is a YYYY-MM-DD date`);
  return Date.parse(`${since}T00:00:00Z`);
}

/** The `since` day a preset names at `now`; `lastVisit` is an ISO time, or null before the first catch-up. */
export function sinceFor(preset: RangePresetId, now: Date, lastVisit: string | null): string {
  invariant(Number.isFinite(now.getTime()), "a range is chosen at a real time");
  if (preset !== "visit") return dayOf(now.getTime() - PRESET_DAYS[preset] * DAY_MS);
  const visited = lastVisit === null ? Number.NaN : Date.parse(lastVisit);
  const usable = Number.isFinite(visited) && visited <= now.getTime();
  return dayOf(usable ? visited : now.getTime() - FIRST_VISIT_DAYS * DAY_MS);
}
