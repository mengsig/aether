// A small map of strings kept in the host's key-value store, for what the client remembers between
// visits: the last catch-up per repository and the session a source started. Storage is untrusted input.

import type { KeyValueStore } from "@aether-ui/protocol";
import { invariant } from "../learn/model.ts";

/** The entries kept per map; the oldest are dropped first. */
export const STRING_MAP_CAP = 200;

/** The stored map, keeping only string-to-string entries; a missing value is an empty map. */
export async function readStringMap(storage: KeyValueStore, key: string): Promise<Record<string, string>> {
  const stored = await storage.get(key);
  if (stored === undefined || stored === null) return {};
  invariant(typeof stored === "object" && !Array.isArray(stored), `${key} holds a map`);
  return Object.fromEntries(Object.entries(stored).filter((entry): entry is [string, string] => typeof entry[1] === "string"));
}

/** Sets one entry, newest last, and writes the map back. */
export async function putInStringMap(storage: KeyValueStore, key: string, name: string, value: string): Promise<void> {
  invariant(name.length > 0 && value.length > 0, "a stored entry has a name and a value");
  const held = Object.entries(await readStringMap(storage, key)).filter(([entryName]) => entryName !== name);
  const kept = [...held, [name, value] as const].slice(-STRING_MAP_CAP);
  invariant(kept.at(-1)?.[0] === name, "the entry just set is the newest");
  await storage.set(key, Object.fromEntries(kept));
}
