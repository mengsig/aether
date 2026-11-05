// The end-to-end suite: real aetherd and archond with a faux engine (harness/), driven through
// Chromium. Run it with scripts/e2e.sh, which resolves AETHER_BIN and ARCHOND_BIN.

import { defineConfig } from "@playwright/test";
import path from "node:path";

const OUT = process.env.E2E_OUT ?? path.join(import.meta.dirname, ".out");

export default defineConfig({
  testDir: "specs",
  outputDir: path.join(OUT, "results"),
  globalSetup: "./harness/global-setup.ts",
  // One worker: the specs share one stack, and restarting a daemon must not race another spec.
  workers: 1,
  fullyParallel: false,
  forbidOnly: true,
  retries: 0,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  reporter: [["list"], ["json", { outputFile: path.join(OUT, "report.json") }]],
  use: {
    browserName: "chromium",
    headless: true,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    viewport: { width: 1280, height: 800 },
  },
});
