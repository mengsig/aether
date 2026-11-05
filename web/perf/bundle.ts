// The bundle-size measures of web/dist: `node perf/bundle.ts [dist dir] [out file]`. Bytes are exact
// and gzip runs at a fixed level, so the numbers are the same on every host for the same build.

import { readdirSync, readFileSync, statSync, writeFileSync, mkdirSync } from "node:fs";
import path from "node:path";
import { gzipSync } from "node:zlib";

type Measures = Record<string, { value: number }>;

function files(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    return entry.isDirectory() ? files(full) : [full];
  });
}

const gzipped = (file: string): number => gzipSync(readFileSync(file), { level: 9 }).length;

/** The script and stylesheet index.html loads before anything else: what the first paint waits for. */
function initialAssets(dist: string): string[] {
  const html = readFileSync(path.join(dist, "index.html"), "utf8");
  const refs = [...html.matchAll(/<(?:script|link)\b[^>]*?(?:src|href)="\.\/(assets\/[^"]+\.(?:js|css))"/g)].map((match) => match[1] ?? "");
  if (!refs.some((ref) => ref.endsWith(".js"))) throw new Error("index.html loads no entry script");
  return refs.map((ref) => path.join(dist, ref));
}

export function bundleMeasures(dist: string): Measures {
  const all = files(dist);
  if (!all.some((file) => path.basename(file) === "index.html")) throw new Error(`${dist} holds no index.html; build web/dist first`);
  const bytes = (list: readonly string[]) => list.reduce((sum, file) => sum + statSync(file).size, 0);
  const js = all.filter((file) => file.endsWith(".js"));
  const css = all.filter((file) => file.endsWith(".css"));
  const initial = initialAssets(dist);
  const largest = Math.max(...js.map((file) => statSync(file).size));
  const measures: Measures = {
    "bundle.total_bytes": { value: bytes(all) },
    "bundle.js_bytes": { value: bytes(js) },
    "bundle.css_bytes": { value: bytes(css) },
    "bundle.initial_bytes": { value: bytes(initial) },
    "bundle.initial_gzip_bytes": { value: initial.reduce((sum, file) => sum + gzipped(file), 0) },
    "bundle.js_gzip_bytes": { value: js.reduce((sum, file) => sum + gzipped(file), 0) },
    "bundle.largest_js_bytes": { value: largest },
  };
  if (measures["bundle.initial_bytes"]?.value === 0 || !Number.isFinite(largest)) throw new Error("the bundle has no initial assets");
  return measures;
}

if (import.meta.main) {
  const dist = path.resolve(process.argv[2] ?? path.join(import.meta.dirname, "..", "dist"));
  const out = path.resolve(process.argv[3] ?? path.join(import.meta.dirname, ".out", "bundle.json"));
  mkdirSync(path.dirname(out), { recursive: true });
  writeFileSync(out, `${JSON.stringify(bundleMeasures(dist), null, 2)}\n`);
}
