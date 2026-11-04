import { invariant } from "../invariant.ts";

/** The value, which a test knows is there. */
export function must<T>(value: T | undefined, what: string): T {
  invariant(value !== undefined, `${what} is missing`);
  return value;
}
