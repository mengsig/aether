// What every journey shares: the variant (JOURNEY_ENGINE=faux|real), and per spec file a fresh
// stack, a private headless sway and the packaged app on the stack's sockets, all stopped after.

import assert from "node:assert/strict";
import { appendFile, mkdtemp, rm } from "node:fs/promises";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { startAudit, type Audit } from "../../harness/audit.ts";
import { tempBase } from "../../harness/env.ts";
import { launchPackaged, packagedExec, startSway, type PackagedApp, type Sway } from "../../harness/packaged.ts";
import { connection } from "../../harness/pair.ts";
import { record, until } from "../../harness/process.ts";
import { checkReadiness, enableRealEngines, readinessMessage } from "../../harness/real-engine.ts";
import { binariesFromEnv, startStack, type Stack } from "../../harness/stack.ts";
import { stackSpend } from "./daemon.ts";
import { journeyScript } from "./scripts.ts";

export type Variant = "faux" | "real";
const SETUP_MS = 300_000;
const READY_MS = 30_000;

/** `JOURNEY_ENGINE`: `faux` (the default, CI) or `real` (the user's own logins, on demand). */
export function journeyVariant(env: NodeJS.ProcessEnv = process.env): Variant {
  const value = env.JOURNEY_ENGINE ?? "faux";
  assert.ok(value === "faux" || value === "real", `JOURNEY_ENGINE is faux or real, not ${value}`);
  return value;
}

export interface Journey {
  readonly variant: Variant;
  readonly stack: Stack;
  readonly sway: Sway;
  readonly app: PackagedApp;
  readonly window: Page;
  /** Page errors the app window threw since the last check. */
  readonly pageErrors: string[];
}

export interface JourneyOptions {
  /** archond too (the default); false is Aether alone. */
  readonly archon?: boolean;
  /** Why this journey does not run on real engines, when it does not. */
  readonly fauxOnly?: string;
  /** The app on Xwayland instead of Wayland: its native dialogs are then driven with xdotool. */
  readonly x11?: boolean;
}

/** Runs every step even when one fails, then fails with all of them. */
async function runAll(what: string, steps: ReadonlyArray<() => Promise<unknown>>): Promise<void> {
  const failures: unknown[] = [];
  for (const step of steps) await step().catch((error: unknown) => { failures.push(error); });
  if (failures.length > 0) throw new AggregateError(failures, `${what} did not stop cleanly: ${failures.map(String).join("; ")}`);
}

interface Parts { stack?: Stack; audit?: Audit; swayRoot?: string; sway?: Sway; app?: PackagedApp }

async function teardown(parts: Parts): Promise<void> {
  await runAll("the journey", [
    async () => parts.app?.close(),
    async () => parts.sway?.stop(),
    async () => { if (parts.swayRoot !== undefined) await rm(parts.swayRoot, { recursive: true, force: true, maxRetries: 3 }); },
    async () => parts.stack?.stop(),
  ]);
  const audit = parts.audit;
  if (audit === undefined) return;
  audit.stop();
  assert.ok(audit.samples > 0, "the journey's hermetic audit sampled");
  if (audit.violations.length > 0) throw new Error(`The hermetic audit saw:\n${audit.violations.join("\n")}`);
}

/** The faux variant is audited like the suite: loopback only, daemon files inside the root. */
function auditOf(stack: Stack): Audit {
  return startAudit({
    root: () => stack.layout.root,
    binaries: [stack.binaries.aether, stack.binaries.archon],
    daemons: () => [stack.aether.pid, stack.archon.pid].filter((pid): pid is number => pid !== undefined),
    clients: () => [process.pid],
  });
}

async function boot(parts: Parts, variant: Variant, options: JourneyOptions): Promise<Journey> {
  const exec = await packagedExec();
  const stack = await startStack(await binariesFromEnv(), { archon: options.archon ?? true });
  parts.stack = stack;
  stack.engine.setScript(journeyScript);
  if (variant === "real") {
    await enableRealEngines(stack);
    const readiness = await checkReadiness(stack);
    if (!readiness.ready) throw new SkipJourney(readinessMessage(readiness));
  } else {
    parts.audit = auditOf(stack);
  }
  parts.swayRoot = await mkdtemp(path.join(tempBase(), "sw-"));
  record("roots", parts.swayRoot);
  const sway = await startSway(parts.swayRoot, { xwayland: options.x11 === true });
  parts.sway = sway;
  const app = await launchPackaged({ exec, sway, aetherSocket: stack.layout.aether.socket, archonSocket: stack.layout.archon.socket });
  parts.app = app;
  const pageErrors: string[] = [];
  app.window.on("pageerror", (error) => pageErrors.push(error.message));
  return { variant, stack, sway, app, window: app.window, pageErrors };
}

class SkipJourney extends Error {}

/** The real variant's spend for one spec file, appended to `spend.jsonl` beside the report. */
async function recordSpend(journey: Journey): Promise<void> {
  const spend = await stackSpend(journey.stack);
  const file = path.basename(test.info().file);
  assert.ok(file.endsWith(".spec.ts"), `spend is recorded per spec file: ${file}`);
  const line = JSON.stringify({ file, at: new Date().toISOString(), ...spend });
  await appendFile(path.join(test.info().project.outputDir, "..", "spend.jsonl"), `${line}\n`);
  console.log(`journey spend: ${line}`);
}

/**
 * Registers the spec file's hooks and returns its journey: started before the first test, stopped
 * after the last. The real variant skips the file, after stopping what it started, when its
 * engines are not ready.
 */
export function journeyHooks(options: JourneyOptions = {}): () => Journey {
  const variant = journeyVariant();
  const parts: Parts = {};
  let journey: Journey | undefined;
  test.describe.configure({ mode: "serial" });
  test.beforeAll(async () => {
    test.setTimeout(SETUP_MS);
    // The base suite's testDir holds this directory too; there the journeys only list as skipped.
    test.skip(process.env.E2E_JOURNEYS !== "1", "journeys run through specs/journeys/playwright.config.ts (scripts/e2e_journeys.sh)");
    test.skip(variant === "real" && options.fauxOnly !== undefined, options.fauxOnly ?? "");
    try {
      journey = await boot(parts, variant, options);
    } catch (error) {
      await teardown(parts);
      if (error instanceof SkipJourney) test.skip(true, error.message);
      throw error;
    }
  });
  test.afterAll(async () => {
    test.setTimeout(SETUP_MS);
    try {
      if (variant === "real" && journey !== undefined) await recordSpend(journey);
    } finally {
      await teardown(parts);
    }
  });
  test.afterEach(async () => {
    if (journey === undefined) return;
    // The window as the test left it, passed or failed, for the run's evidence.
    const shot = test.info().outputPath("window.png");
    await journey.window.screenshot({ path: shot, timeout: 15_000 });
    await test.info().attach("window at the end", { path: shot, contentType: "image/png" });
    expect(journey.pageErrors.splice(0), "the app window threw no page error").toEqual([]);
  });
  return () => {
    assert.ok(journey !== undefined, "the journey started in beforeAll");
    return journey;
  };
}

/** The app window's connections to the stack's daemons are ready (local sockets need no pairing). */
export async function connectionsReady(journey: Journey, archon = true): Promise<void> {
  await expect(connection(journey.window, "aether"), "the Aether connection is ready").toHaveAttribute("data-state", "ready", { timeout: READY_MS });
  if (archon) await expect(connection(journey.window, "archon"), "the Archon connection is ready").toHaveAttribute("data-state", "ready", { timeout: READY_MS });
}

/**
 * Pairs the window with aetherd when it asks, with a code the host CLI opens as pair.ts does.
 * The packaged app on local sockets does not ask; then this returns false.
 */
export async function pairIfAsked(journey: Journey): Promise<boolean> {
  const { window, stack } = journey;
  const code = window.getByLabel("Pairing code");
  const ready = connection(window, "aether").and(window.locator("[data-state=ready]"));
  await expect(ready.or(code).first(), "the window connects or asks to pair").toBeVisible({ timeout: READY_MS });
  if (!(await code.isVisible())) return false;
  const pairing = await stack.aether.pairing("operator");
  await code.fill(pairing.code);
  await window.getByLabel("Name for this device").fill("Journey desktop");
  await window.getByRole("button", { name: "Pair" }).click();
  const claimed = await until(() => stack.aether.request({ cmd: "get_pairing", id: pairing.id }), (reply) => reply.data.state === "claimed", "the window's claim", READY_MS);
  assert.equal(claimed.data.state, "claimed");
  const enrolled = await stack.aether.request({ cmd: "confirm_pairing", id: pairing.id, accept: true });
  assert.equal(enrolled.data.state, "enrolled", "the host confirms the window's pairing");
  return true;
}
