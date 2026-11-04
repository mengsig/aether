// Runtime checks for values that cross a daemon, bridge or storage boundary.

export type Json = Record<string, unknown>;

/** Throws when an invariant the caller relies on does not hold. */
export function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`Invariant failed: ${message}`);
}

export function object(value: unknown, label: string): Json {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new TypeError(`${label} must be an object`);
  if (Object.hasOwn(value, "__proto__")) throw new TypeError(`${label} contains an unsafe key`);
  return value as Json;
}

export function string(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0) throw new TypeError(`${label} must be a nonempty string`);
  return value;
}

export function seq(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) throw new TypeError(`${label} must be a nonnegative safe integer`);
  return value;
}

export function array(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) throw new TypeError(`${label} must be an array`);
  return value;
}

export function strings(value: unknown, label: string): string[] {
  return array(value ?? [], label).map((item, index) => string(item, `${label}[${index}]`));
}

/** A copy of a record without one key. */
export function omit<T>(record: Readonly<Record<string, T>>, key: string): Record<string, T> {
  return Object.fromEntries(Object.entries(record).filter(([name]) => name !== key));
}

export const errorText = (error: unknown): string => (error instanceof Error ? error.message : String(error));
