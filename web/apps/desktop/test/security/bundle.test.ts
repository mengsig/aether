// The desktop app is the web app: app:// serves the committed web/dist byte for byte, the same
// directory the remote listener embeds, and the desktop package holds no UI of its own.

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { serveApp } from "../../src/main/app-protocol.ts";
import { DESKTOP_ROOT, REPO_ROOT, WEB_DIST } from "../../src/main/paths.ts";
import { APP_CSP } from "../../src/security/csp.ts";
import { bundleFiles } from "./bundle-files.ts";

const get = (url: string, method = "GET") => serveApp(WEB_DIST, new Request(url, { method }));

test("app:// returns every web/dist file byte for byte, with the CSP", async () => {
  const files = bundleFiles();
  assert.ok(files.includes("index.html") && files.includes("sw.js") && files.includes("manifest.webmanifest"));
  for (const file of files) {
    const response = await get(`app://aether/${file}`);
    assert.equal(response.status, 200, file);
    assert.equal(response.headers.get("content-security-policy"), APP_CSP, file);
    assert.ok(Buffer.from(await response.arrayBuffer()).equals(readFileSync(path.join(WEB_DIST, file))), file);
  }
  const root = Buffer.from(await (await get("app://aether/")).arrayBuffer());
  assert.ok(root.equals(readFileSync(path.join(WEB_DIST, "index.html"))));
});

test("index.html is the document the listener embeds and serves", () => {
  const selection = readFileSync(path.join(REPO_ROOT, "crates/aether-remote/src/assets/selection.rs"), "utf8");
  const bundleDir = /pub const BUNDLE_DIR: &str = "([^"]+)";/.exec(selection)?.[1];
  assert.ok(bundleDir !== undefined, "selection.rs names the bundle directory");
  assert.equal(path.resolve(REPO_ROOT, "crates/aether-remote", bundleDir), WEB_DIST);
  const required = [...(/pub const REQUIRED: &\[&str\] = &\[([^\]]+)\]/.exec(selection)?.[1] ?? "").matchAll(/"\/([^"]+)"/g)]
    .map((match) => match[1]);
  assert.ok(required.includes("index.html"));
  for (const file of required) assert.ok(bundleFiles().includes(file ?? ""), `${file} is in web/dist`);
});

test("app:// serves nothing outside web/dist and only reads", async () => {
  for (const url of ["app://aether/../package.json", "app://aether/%2e%2e/package.json", "app://aether/.hidden",
    "app://aether/assets/..%2f..%2fpackage.json", "app://evil/index.html", "app://aether/nope.js", "app://aether/assets"]) {
    assert.equal((await get(url)).status, 404, url);
  }
  assert.equal((await get("app://aether/index.html", "POST")).status, 405);
});

const UI_FILE = /\.(html?|css|scss|less|jsx|tsx|vue|svelte|map)$/i;

function desktopFiles(dir = DESKTOP_ROOT): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (entry.name === "node_modules") return [];
    const file = path.join(dir, entry.name);
    return entry.isDirectory() ? desktopFiles(file) : [path.relative(DESKTOP_ROOT, file)];
  });
}

test("the desktop package has no HTML, CSS or renderer bundle of its own", () => {
  const files = desktopFiles();
  assert.ok(files.includes("package.json") && files.length > 10, "the walk saw the package");
  assert.deepEqual(files.filter((file) => UI_FILE.test(file)), []);
  assert.deepEqual(files.filter((file) => /\.(m?js)$/.test(file)), [], "no bundled JavaScript");
  assert.deepEqual(files.filter((file) => file.endsWith(".cjs")), [path.join("src", "preload", "index.cjs")]);
  assert.ok(!files.some((file) => file.split(path.sep).includes("dist")), "no build output");
});
