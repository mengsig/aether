// Screenshots of the packaged app's window at a journey's key moments, for the design report:
// light and dark, the whole page, under $E2E_CAPTURE/desktop with one manifest line per shot.
// Without E2E_CAPTURE a shot is a no-op, so the journeys' checks are the same either way.

import assert from "node:assert/strict";
import { appendFile, mkdir } from "node:fs/promises";
import path from "node:path";
import { test, type Page } from "@playwright/test";

const OUT = process.env.E2E_CAPTURE === undefined ? undefined : path.join(process.env.E2E_CAPTURE, "desktop");
const SCHEMES = ["light", "dark"] as const;
const SETTLE_MS = 350;
const SHOT_MS = 15_000;

/** Shoots the window as `screen` in `state`, in both colour schemes, and leaves the scheme as it was. */
export async function shootWindow(window: Page, screen: string, state: string): Promise<void> {
  assert.ok(screen.trim() !== "" && state.trim() !== "", "a shot names its screen and state");
  if (OUT === undefined) return;
  await mkdir(OUT, { recursive: true });
  const journey = path.basename(test.info().file, ".spec.ts");
  const file = `${journey}-${screen}-${state}`.toLowerCase().replace(/[^a-z0-9-]+/g, "-");
  // Off every control, so no hover style shows in the shot.
  await window.mouse.move(0, 0);
  try {
    for (const scheme of SCHEMES) {
      await window.emulateMedia({ colorScheme: scheme, reducedMotion: "reduce" });
      await window.waitForTimeout(SETTLE_MS);
      await window.screenshot({ path: path.join(OUT, `${file}--desktop-${scheme}.png`), fullPage: true, timeout: SHOT_MS });
    }
  } finally {
    await window.emulateMedia({ colorScheme: null, reducedMotion: null });
  }
  const shot = { screen, state, file, journey, variants: SCHEMES.map((scheme) => `desktop-${scheme}`) };
  await appendFile(path.join(OUT, "manifest.jsonl"), `${JSON.stringify(shot)}\n`);
}
