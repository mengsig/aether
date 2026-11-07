/// <reference lib="dom" />
// Screenshots of the Learn screens over their fixture (packages/noesis/src/learn/fixture) and its faux
// Noesis daemon, light and dark. Usage: node dev/learn-capture.ts <out-dir>

import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { chromium, type Page } from "@playwright/test";
import { createServer } from "vite";
import { packageAliases, WEB_ROOT } from "../scripts/aliases.ts";

const out = path.resolve(process.argv[2] ?? "learn-capture");
mkdirSync(out, { recursive: true });

/** Each shot: its file name, the fixture's query and hash, and a selector that shows the state is drawn. */
const SHOTS = [
  { name: "sessions", query: "", hash: "#/learn", ready: "h1" },
  { name: "lesson-graded", query: "graded", hash: "#/learn/sessions/ns_raft", ready: ".learn-q[data-state=correct], .learn-q[data-state=incorrect]" },
  { name: "lesson-question-open", query: "pose=vote", hash: "#/learn/sessions/ns_raft", ready: ".learn-q[data-state=open]" },
  { name: "settings", query: "", hash: "#/learn?panel=settings", ready: ".learn-settings" },
  { name: "knowledge", query: "", hash: "#/learn/graph", ready: ".learn-graph-canvas[data-settled]" },
] as const;

const server = await createServer({
  root: path.join(WEB_ROOT, "packages", "noesis", "src", "learn", "fixture"),
  resolve: { alias: packageAliases }, server: { port: 0, host: "127.0.0.1" }, logLevel: "warn",
});
await server.listen();
const base = server.resolvedUrls?.local[0];
assert.ok(base !== undefined, "the dev server reported a URL");

async function shoot(page: Page, shot: (typeof SHOTS)[number], theme: "light" | "dark"): Promise<void> {
  const query = [shot.query, `theme=${theme}`].filter((part) => part !== "").join("&");
  await page.goto(`${base}?${query}${shot.hash}`);
  await page.locator(shot.ready).first().waitFor({ timeout: 15_000 });
  await page.waitForTimeout(800);
  const file = path.join(out, `${shot.name}--${theme}.png`);
  await page.screenshot({ path: file, fullPage: true });
  console.log(file);
}

const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 1360, height: 900 } });
  for (const theme of ["light", "dark"] as const) {
    for (const shot of SHOTS) await shoot(page, shot, theme);
  }
} finally {
  await browser.close();
  await server.close();
}
