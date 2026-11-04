// Fills the service worker's precache list and version from the finished bundle,
// and fails the build if a test-only package reached it.

import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import type { Plugin, Rollup } from "vite";

export const SW_FILE = "sw.js";
const LIST = "__AETHER_PRECACHE__";
const VERSION = "__AETHER_VERSION__";
// Dev-only licence exceptions (scripts/licenses.ts) and the test DOM: never shipped.
export const TEST_ONLY = ["axe-core", "happy-dom", "minimatch", "caniuse-lite"];

/** Replaces each quoted placeholder with a literal; a missing placeholder is a broken build. */
export function fillPlaceholder(code: string, name: string, literal: string): string {
  const pattern = new RegExp(`(["'\`])${name}\\1`, "g");
  const count = code.match(pattern)?.length ?? 0;
  if (count === 0) throw new Error(`${SW_FILE} lost its ${name} placeholder`);
  const filled = code.replace(pattern, () => literal);
  if (filled.includes(name)) throw new Error(`${SW_FILE} still holds ${name}`);
  return filled;
}

/** The files a worker precaches: everything built except itself and source maps, the shell first. */
export function precacheList(files: readonly string[]): string[] {
  const assets = files.filter((file) => file !== SW_FILE && !file.endsWith(".map")).sort();
  if (!assets.includes("index.html")) throw new Error("the bundle has no index.html to precache");
  return ["./", ...assets.filter((file) => file !== "index.html").map((file) => `./${file}`)];
}

function testOnlyModule(bundle: Rollup.OutputBundle): string | undefined {
  for (const output of Object.values(bundle)) {
    if (output.type !== "chunk") continue;
    const hit = Object.keys(output.modules).find((id) => TEST_ONLY.some((name) => id.includes(`/node_modules/${name}/`)));
    if (hit) return `${output.fileName} bundles ${hit}`;
  }
  return undefined;
}

/** Every file under the public directory, as bundle-relative paths. */
function publicFiles(dir: string): string[] {
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => path.relative(dir, path.join(entry.parentPath, entry.name)).split(path.sep).join("/"));
}

export function serviceWorkerPlugin(): Plugin {
  let publicDir = "";
  return {
    name: "aether-service-worker",
    configResolved(config) {
      publicDir = config.publicDir;
    },
    apply: "build",
    enforce: "post",
    // After Vite has emitted index.html, so the list is the whole bundle.
    generateBundle: { order: "post", handler(_options, bundle) {
      const leak = testOnlyModule(bundle);
      if (leak) this.error(`A test-only package reached the shipped bundle: ${leak}`);
      const worker = bundle[SW_FILE];
      if (worker?.type !== "chunk") this.error(`the build produced no ${SW_FILE} chunk`);
      if (publicDir === "") this.error("the public directory is not configured");
      const extra = publicFiles(publicDir);
      const list = precacheList([...Object.keys(bundle), ...extra]);
      // Over every byte the worker caches, so a changed icon or manifest also starts a new cache.
      const hash = createHash("sha256");
      for (const [name, output] of Object.entries(bundle).sort(([a], [b]) => a.localeCompare(b))) {
        if (name !== SW_FILE) hash.update(name).update(output.type === "chunk" ? output.code : output.source);
      }
      for (const file of extra.sort()) hash.update(file).update(readFileSync(path.join(publicDir, file)));
      const version = hash.digest("hex").slice(0, 16);
      worker.code = fillPlaceholder(fillPlaceholder(worker.code, LIST, JSON.stringify(list)), VERSION, JSON.stringify(version));
    } },
  };
}
