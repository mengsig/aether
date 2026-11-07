// A small seeded generator (mulberry32): the same seed always gives the same fixture, on every
// host, so a budget compares like with like and no fixture blob is committed.

export interface Random {
  /** A float in [0, 1). */
  next(): number;
  /** A whole number in [0, bound). */
  below(bound: number): number;
  pick<T>(choices: readonly T[]): T;
}

export function seeded(seed: number): Random {
  if (!Number.isInteger(seed) || seed < 0 || seed > 0xffff_ffff) throw new Error(`seed ${seed} is not a 32-bit whole number`);
  let state = seed;
  const next = (): number => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const below = (bound: number): number => {
    if (!Number.isInteger(bound) || bound <= 0) throw new Error(`bound ${bound} is not a positive whole number`);
    return Math.floor(next() * bound);
  };
  const pick = <T>(choices: readonly T[]): T => {
    const choice = choices[below(choices.length)];
    if (choice === undefined) throw new Error("pick needs at least one choice");
    return choice;
  };
  return { next, below, pick };
}

export const pad = (n: number, width = 6): string => String(n).padStart(width, "0");

const T0 = Date.parse("2026-11-04T08:00:00Z");
/** A fixed clock: `seconds` after the fixtures' epoch, as the daemon's RFC 3339 stamps. */
export const stamp = (seconds: number): string => new Date(T0 + seconds * 1000).toISOString();
