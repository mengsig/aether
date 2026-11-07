// Before the suite: the user's own Aether and Archon config dirs as they are. After it: the
// same, byte sizes and mtimes included, or the run fails. The stacks never point there.

import { diffSnapshots, snapshotUserConfig } from "./audit.ts";

export default async function globalSetup(): Promise<() => Promise<void>> {
  const before = await snapshotUserConfig();
  if (before.size === 0) throw new Error("the snapshot names at least the two config dirs, present or absent");
  return async () => {
    const changes = diffSnapshots(before, await snapshotUserConfig());
    if (changes.length > 0) throw new Error(`The suite changed the user's own config:\n${changes.join("\n")}`);
    console.log(`Hermetic: ${before.size} entries of the user's Aether and Archon config are unchanged.`);
  };
}
