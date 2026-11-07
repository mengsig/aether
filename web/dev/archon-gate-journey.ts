/// <reference lib="dom" />
// Exercises the long proposal-gate fixture in a browser. Pass an absolute directory for screenshots.
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { chromium } from "@playwright/test";
import { createServer } from "vite";
import { packageAliases, WEB_ROOT } from "../scripts/aliases.ts";

const out = process.argv[2];
assert.ok(out && path.isAbsolute(out), "the screenshot directory is absolute");
await mkdir(out, { recursive: true });
const root = path.join(WEB_ROOT, "packages/archon/src/queue/fixture");
const server = await createServer({ root, configFile: false, logLevel: "warn", resolve: { alias: packageAliases },
  server: { host: "127.0.0.1", port: 0, fs: { allow: [WEB_ROOT] } } });
try {
  await server.listen();
  const address = server.httpServer?.address();
  assert.ok(address && typeof address === "object", "the fixture listens on a local port");
  const browser = await chromium.launch({ headless: true });
  try {
    for (const theme of ["light", "dark"] as const) {
      const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, colorScheme: theme });
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.goto(`http://127.0.0.1:${address.port}/?gate=long&theme=${theme}#/archon/items/qi_pairing`);
      const gate = page.locator('section[aria-label="Gates"]');
      const details = gate.locator("details");
      await details.waitFor();
      assert.equal(await details.evaluate((element) => (element as HTMLDetailsElement).open), false);
      await gate.scrollIntoViewIfNeeded();
      await page.screenshot({ path: path.join(out, `gate-collapsed-${theme}.png`) });
      await details.locator("summary").click();
      assert.equal(await details.evaluate((element) => (element as HTMLDetailsElement).open), true);
      const text = gate.locator(".item-gate-content");
      assert.equal(await text.evaluate((element) => element.scrollWidth <= element.clientWidth), true, "the long path stays inside its card");
      assert.match(await text.innerText(), /long-path/);
      await page.screenshot({ path: path.join(out, `gate-expanded-${theme}.png`) });
      assert.deepEqual(errors, [], "the fixture has no page errors");
      await page.close();
    }
  } finally {
    await browser.close();
  }
} finally {
  await server.close();
}
