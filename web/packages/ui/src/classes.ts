/** Joins the truthy class names, so optional `className` props compose. */
export function classes(...names: ReadonlyArray<string | false | null | undefined>): string {
  return names.filter((name): name is string => typeof name === "string" && name.length > 0).join(" ");
}

/** Throws when an invariant the kit relies on is broken; these are programmer errors. */
export function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`@aether-ui/ui: ${message}`);
}
