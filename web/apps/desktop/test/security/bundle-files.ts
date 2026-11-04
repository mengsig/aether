// The shipped files of web/dist as the listener's build script collects them: hidden files skipped.

import { readdirSync } from "node:fs";
import path from "node:path";
import { WEB_DIST } from "../../src/main/paths.ts";

export function bundleFiles(dir = WEB_DIST, prefix = ""): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (entry.name.startsWith(".")) return [];
    const relative = `${prefix}${entry.name}`;
    return entry.isDirectory() ? bundleFiles(path.join(dir, entry.name), `${relative}/`) : [relative];
  });
}
