// What a real listener serves: exactly the committed web/dist, under the remote security
// headers, and nothing of the retired phone app.

import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";

export const DIST = path.resolve(import.meta.dirname, "..", "..", "dist");

/** Paths of the retired crates/aether-remote/web app; no route may answer them now. */
const LEGACY = ["/app.js", "/boot.js", "/precache.js", "/push-renewal.js", "/styles.css", "/asset-manifest.json",
  "/ui/shell.js", "/core/transport.js", "/features/start/pair.js", "/assets/icons/icon-192.png"];
const FRAME = "/mermaid-frame.html";
const FRAME_CSP = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'none'; img-src data:; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";

/** Every file of the bundle, as URL paths: what the binary must have embedded. */
export async function bundleFiles(): Promise<string[]> {
  const entries = await readdir(DIST, { recursive: true, withFileTypes: true });
  const files = entries
    .filter((entry) => entry.isFile() && !entry.name.startsWith("."))
    .map((entry) => `/${path.relative(DIST, path.join(entry.parentPath, entry.name)).split(path.sep).join("/")}`)
    .sort();
  assert.ok(files.includes("/index.html") && files.includes("/sw.js") && files.includes(FRAME), `web/dist is a built bundle: ${files.join(", ")}`);
  return files;
}

async function get(url: string): Promise<Response> {
  return fetch(url, { signal: AbortSignal.timeout(10_000) });
}

/** Each bundle file is served byte for byte with the listener's CSP; legacy paths are 404. Returns the CSP. */
export async function checkBundle(origin: string): Promise<string> {
  const files = await bundleFiles();
  let policy: string | null = null;
  for (const file of files) {
    const response = await get(`${origin}${file}`);
    assert.equal(response.status, 200, file);
    const served = Buffer.from(await response.arrayBuffer());
    assert.ok(served.equals(await readFile(path.join(DIST, file))), `${file} is served as committed`);
    const csp = response.headers.get("content-security-policy");
    if (file === FRAME) {
      assert.equal(csp, FRAME_CSP, "the renderer frame has its restricted policy");
      assert.equal(response.headers.get("x-frame-options"), "SAMEORIGIN", file);
    } else {
      policy ??= csp;
      assert.equal(csp, policy, `${file} has the main CSP`);
      assert.equal(response.headers.get("x-frame-options"), "DENY", file);
    }
    assert.equal(response.headers.get("x-content-type-options"), "nosniff", file);
  }
  assert.ok(policy !== null, "every asset carries a CSP");
  const directives = policy.split(";").map((part) => part.trim());
  assert.ok(directives.includes("connect-src 'self'") && !policy.includes("unsafe"), policy);
  const shell = await readFile(path.join(DIST, "index.html"));
  const route = await get(`${origin}/pair`);
  assert.ok(route.status === 200 && Buffer.from(await route.arrayBuffer()).equals(shell), "/pair is the shell");
  for (const legacy of LEGACY) {
    const response = await get(`${origin}${legacy}`);
    assert.equal(response.status, 404, `${legacy} is gone`);
    await response.body?.cancel();
  }
  console.log(`Live bundle: ${files.length} files of web/dist served as committed; ${LEGACY.length} legacy paths 404.`);
  return policy;
}
