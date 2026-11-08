// The packaged-app journeys (docs/e2e-journeys.md): the base suite's settings with this
// directory as testDir, longer timeouts, and their own output. Run through scripts/e2e_journeys.sh.

import path from "node:path";
import { defineConfig } from "@playwright/test";
import base from "../../playwright.config.ts";

// The journeys' specs skip themselves unless this config loaded them (see journey.ts).
process.env.E2E_JOURNEYS = "1";
const OUT = path.join(process.env.E2E_OUT ?? path.join(import.meta.dirname, "..", "..", ".out"), "journeys");

export default defineConfig({
  ...base,
  testDir: ".",
  testMatch: "*.spec.ts",
  outputDir: path.join(OUT, "results"),
  globalSetup: path.join(import.meta.dirname, "..", "..", "harness", "global-setup.ts"),
  // One worker: one packaged app and one sway at a time.
  workers: 1,
  timeout: 240_000,
  expect: { timeout: 20_000 },
  reporter: [["list"], ["json", { outputFile: path.join(OUT, "report.json") }]],
});
