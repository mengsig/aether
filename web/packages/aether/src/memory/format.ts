// Small text formats shared by the memory views: daemon timestamps, sizes, scores and shares.

import { invariant } from "../fleet/invariant.ts";

/** "2026-11-05 07:31 UTC" for an RFC 3339 stamp; anything else is shown as the daemon sent it. */
export function when(stamp: string): string {
  const match = /^(\d{4}-\d\d-\d\d)T(\d\d:\d\d)(?::\d\d(?:\.\d+)?)?Z$/.exec(stamp);
  return match ? `${match[1]} ${match[2]} UTC` : stamp;
}

/** A byte count in binary units, one decimal past a kibibyte. */
export function bytes(count: number): string {
  invariant(Number.isFinite(count) && count >= 0, "a byte count is a non-negative number");
  const units = ["B", "KiB", "MiB", "GiB"];
  let value = count;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return unit === 0 ? `${value} B` : `${value.toFixed(1)} ${units[unit]}`;
}

/** A retrieval score or relevance to two places. */
export function score(value: number): string {
  invariant(Number.isFinite(value), "a score is a finite number");
  return value.toFixed(2);
}

/** A rate in [0, 1] as a percentage. */
export function share(rate: number): string {
  invariant(Number.isFinite(rate) && rate >= 0 && rate <= 1, "a rate lies in [0, 1]");
  return `${Math.round(rate * 1000) / 10}%`;
}

/** "1 memory", "3 memories". */
export function count(n: number, one: string, many: string): string {
  invariant(Number.isInteger(n) && n >= 0, "a count is a whole number");
  return `${n} ${n === 1 ? one : many}`;
}
