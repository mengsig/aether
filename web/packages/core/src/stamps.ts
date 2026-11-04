// Ordering of RFC 3339 stamps as both daemons write them.

import { invariant } from "./guards.ts";

const STAMP = /^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?(Z|[+-]\d\d:\d\d)$/i;

/** Orders two RFC 3339 stamps to the nanosecond; Date.parse alone stops at the millisecond. */
export function compareStamps(a: string, b: string): number {
  const [left, right] = [stampParts(a), stampParts(b)];
  return left.seconds - right.seconds || left.nanos - right.nanos;
}

function stampParts(stamp: string): { seconds: number; nanos: number } {
  const match = STAMP.exec(stamp);
  invariant(match !== null, `${stamp} is an RFC 3339 timestamp`);
  const seconds = Date.parse(`${match[1]}${match[3]}`);
  invariant(Number.isFinite(seconds), `${stamp} names a real instant`);
  return { seconds, nanos: Number((match[2] ?? "").padEnd(9, "0")) };
}
