import { invariant } from "./classes.ts";

export const MAX_RECENTS = 8;

/** `id` first, the rest as before without it, at most `max` long. */
export function pushRecent(recents: readonly string[], id: string, max: number = MAX_RECENTS): string[] {
  invariant(id.length > 0, "a recent destination has an id");
  return [id, ...recents.filter((held) => held !== id)].slice(0, max);
}

