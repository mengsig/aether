/** Throws when a programmer assumption fails; expected bad input is handled where it arrives. */
export function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`@aether-ui/aether: ${message}`);
}
