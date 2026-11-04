// Capped exponential backoff with equal jitter: half fixed, half random, so
// reconnecting clients spread out yet never retry instantly.

import { invariant } from "./guards.ts";

export const BACKOFF = Object.freeze({ baseMs: 500, capMs: 30_000 });

export function backoffDelay(attempt: number, random: () => number = Math.random): number {
  invariant(Number.isSafeInteger(attempt) && attempt >= 0, `attempt must be a nonnegative integer, got ${attempt}`);
  const r = random();
  invariant(r >= 0 && r < 1, `random() must be in [0, 1), got ${r}`);
  const ceiling = Math.min(BACKOFF.capMs, BACKOFF.baseMs * 2 ** Math.min(attempt, 30));
  return Math.round(ceiling / 2 + (r * ceiling) / 2);
}
