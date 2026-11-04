/** Throws when a programmer assumption is violated; callers validate user input separately. */
export function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`@aether-ui/graph: ${message}`);
}

/** `items[index]`, which must exist: an index out of range is a programmer error. */
export function at<T>(items: ArrayLike<T>, index: number): T {
  const item = items[index];
  if (item === undefined) throw new Error(`@aether-ui/graph: index ${index} is outside 0..${items.length - 1}`);
  return item;
}

/** `map.get(key)`, which must exist. */
export function must<K, V>(map: ReadonlyMap<K, V>, key: K): V {
  const value = map.get(key);
  if (value === undefined) throw new Error(`@aether-ui/graph: missing entry ${String(key)}`);
  return value;
}
