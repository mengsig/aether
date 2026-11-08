/// <reference lib="dom" />
// Builds the shared renderer fixture and checks real Mermaid, MathML and the production CSP.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { chromium, type Page } from "@playwright/test";
import { build } from "vite";
import { packageAliases, WEB_ROOT } from "../scripts/aliases.ts";

const output = path.join(WEB_ROOT, "e2e/.out/markdown");
const CSP = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; manifest-src 'self'; worker-src 'self'; connect-src 'self'; frame-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const FRAME_CSP = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src data:; font-src 'self'; connect-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";
const TYPES: Readonly<Record<string, string>> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };

async function assertRendered(page: Page): Promise<void> {
  await page.waitForFunction(() => [...document.querySelectorAll<HTMLImageElement>(".md-diagram-image")].filter((image) => image.complete && image.naturalWidth > 0).length === 2);
  assert.equal(await page.locator("math").count(), 6, "native MathML is present including rejected trust command text");
  assert.equal(await page.locator(".md-diagram-note").count(), 2, "parse failure and hostile source are visible");
  assert.equal(await page.locator(".md-diagram-measure").count(), 0, "measurement DOM is removed");
  assert.equal(await page.locator(".md-diagram style, .md-diagram [style]").count(), 0, "diagram styles stay out of the app document");
  assert.equal(await page.locator(".md script,.md svg,.md a[href^='javascript:']").count(), 0, "lesson content cannot inject active markup");
  const frame = page.frames().find((candidate) => candidate.url().endsWith("/mermaid-frame.html"));
  assert.ok(frame !== undefined, "the diagram renderer has a separate frame");
  assert.equal(await page.locator("iframe.md-diagram-frame").getAttribute("sandbox"), "allow-scripts allow-same-origin", "the renderer has only the required sandbox permissions");
  assert.ok((await page.locator(".md").textContent())?.includes("Prices $5 and $10"));
  const image = page.locator(".md-diagram-image").first();
  assert.ok((await image.boundingBox())?.width, "the SVG image has visible layout");
}

async function check(url: string): Promise<void> {
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 1200 } });
    const errors: string[] = [];
    const remote: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("console", (message) => { if (message.type() === "error") errors.push(message.text()); });
    page.on("request", (request) => { if (!request.url().startsWith(url) && !request.url().startsWith("data:")) remote.push(request.url()); });
    await page.goto(url);
    await assertRendered(page);
    await page.screenshot({ path: path.join(output, "light.png"), fullPage: true });
    const light = await page.locator(".md-diagram-image").first().getAttribute("src");
    await page.getByRole("button", { name: "Toggle theme" }).click();
    await page.waitForFunction((before) => document.querySelector(".md-diagram-image")?.getAttribute("src") !== before, light);
    await assertRendered(page);
    await page.screenshot({ path: path.join(output, "dark.png"), fullPage: true });
    assert.equal(errors.length, 0, `the renderer raises no browser errors; first: ${errors[0] ?? "none"}`);
    assert.deepEqual(remote, [], "lesson content never fetches a remote resource");
    console.log("markdown browser: real diagrams, MathML, source fallbacks, light/dark theme and CSP passed");
  } finally {
    await browser.close();
  }
}

await build({ root: path.join(WEB_ROOT, "packages/ui/src/markdown/fixture"), configFile: false, logLevel: "warn", resolve: { alias: packageAliases }, build: { outDir: output, emptyOutDir: true } });
await build({ root: path.join(WEB_ROOT, "apps/web"), configFile: false, logLevel: "warn", resolve: { alias: packageAliases }, build: { outDir: output, emptyOutDir: false, rollupOptions: { input: path.join(WEB_ROOT, "apps/web/mermaid-frame.html") } } });
const server = createServer((request, response) => {
  const pathname = new URL(request.url ?? "/", "http://localhost").pathname;
  const file = path.resolve(output, `.${pathname === "/" ? "/index.html" : pathname}`);
  if (!file.startsWith(`${output}${path.sep}`)) { response.writeHead(403).end(); return; }
  void readFile(file).then((body) => {
    response.writeHead(200, { "Content-Type": TYPES[path.extname(file)] ?? "application/octet-stream", "Content-Security-Policy": pathname === "/mermaid-frame.html" ? FRAME_CSP : CSP,
      "X-Frame-Options": pathname === "/mermaid-frame.html" ? "SAMEORIGIN" : "DENY",
      "Cross-Origin-Resource-Policy": "same-origin" });
    response.end(body);
  }, (error: unknown) => { response.writeHead(404).end(String(error)); });
});
await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
try {
  const address = server.address();
  assert.ok(address !== null && typeof address === "object", "fixture server has a port");
  await check(`http://127.0.0.1:${address.port}/`);
} finally {
  await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
}
