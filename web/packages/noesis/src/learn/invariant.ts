// The package's one invariant, in a file of its own: modules the shell loads eagerly (the
// understand actions) import it without pulling the learning screens' model into the entry chunk.

export function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`Invariant failed: ${message}`);
}
