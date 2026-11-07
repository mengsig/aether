// The live acceptance of the served frontend: `node dev/live/run.ts /absolute/path/to/aether`.
// It never builds Rust: it checks the binary it is given, which embeds web/dist as committed.

import assert from "node:assert/strict";
import { access, constants } from "node:fs/promises";
import path from "node:path";
import { checkBundle } from "./bundle.ts";
import { startDaemon } from "./daemon.ts";
import { checkBrowserPairing, checkClaims } from "./pairing.ts";

const binary = process.argv[2] ?? process.env.AETHER_BINARY;
if (binary === undefined || binary === "") throw new Error("usage: node dev/live/run.ts /absolute/path/to/aether");
await access(binary, constants.X_OK);

/** A missing browser skips the browser half, loudly; any other launch failure fails the run. */
async function launchChromium() {
  const { chromium } = await import("@playwright/test");
  try {
    return await chromium.launch({ headless: true });
  } catch (error) {
    if (!(error instanceof Error) || !error.message.includes("Executable doesn't exist")) throw error;
    console.log("\n\x1b[1;33m==> Live browser pairing SKIPPED: no Chromium (npx playwright install chromium in web/).\x1b[0m\n");
    return null;
  }
}

const daemon = await startDaemon(path.resolve(binary));
try {
  const policy = await checkBundle(daemon.url);
  await checkClaims(daemon);
  const browser = await launchChromium();
  if (browser !== null) {
    try {
      const named = new URL(daemon.url);
      named.hostname = "localhost";
      for (const origin of [daemon.url, named.origin]) await checkBrowserPairing(browser, daemon, origin);
    } finally {
      await browser.close();
    }
  }
  assert.ok(policy.length > 0);
  console.log(`Live acceptance passed. CSP: ${policy}`);
} finally {
  await daemon.close();
}
