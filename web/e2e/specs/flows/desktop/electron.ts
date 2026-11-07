// The real Electron desktop app for the desktop flows: web/apps/desktop/test/e2e/main.ts runs
// startDesktop offscreen on a stack's sockets, headless, and Playwright drives its windows.

import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import { _electron, expect, type ElectronApplication, type Page } from "@playwright/test";
import { tempBase } from "../../../harness/env.ts";
import { record, startTime } from "../../../harness/process.ts";

const DESKTOP = path.resolve(import.meta.dirname, "..", "..", "..", "..", "apps", "desktop");
const MAIN = path.join(DESKTOP, "test", "e2e", "main.ts");
const LAUNCH_MS = 60_000;
/** One launch normally takes about a second; past this, Playwright's ready handshake has hung. */
const HANDSHAKE_MS = 30_000;
/**
 * Now and then the first launch in a fresh worker hangs after both inspectors connect, before
 * Playwright releases Electron's ready event; a second launch is the one, logged, retry.
 */
const LAUNCH_ATTEMPTS = 2;
/** The app window's own size (app-window.ts); the headless screen clamps windows to their minimum. */
const DESKTOP_VIEWPORT = { width: 1280, height: 820 } as const;
/** Variables that would put Electron on a real display or turn it into plain node. */
const DROPPED = ["DISPLAY", "WAYLAND_DISPLAY", "ELECTRON_RUN_AS_NODE"] as const;

/** A remote origin whose partition trusts one test certificate, and nothing else. */
export interface Trust { readonly origin: string; readonly fingerprint: string }

export interface DesktopLaunch {
  readonly aetherSocket: string;
  readonly archonSocket: string;
  readonly trust?: Trust;
}

/** A confirmation main showed, as security/confirm.ts words it. */
export interface Shown { readonly title: string; readonly message: string; readonly detail?: string; readonly confirm: string }

export interface DesktopApp {
  readonly electron: ElectronApplication;
  /** The app:// window, which reaches the daemons only through window.desktop. */
  readonly window: Page;
  /** Every bridge call the app window made, as main received it. */
  calls(): Promise<Array<{ method: string; args: unknown[] }>>;
  confirmations(): Promise<Shown[]>;
  /** Queues answers for the next native confirmations; with none queued main refuses. */
  answer(...answers: boolean[]): Promise<void>;
  /** Makes main refuse the window's next request for `cmd`, as the daemon would, without sending it. */
  refuseNext(cmd: string, message: string): Promise<void>;
  close(): Promise<void>;
}

function electronBinary(): string {
  const binary = createRequire(path.join(DESKTOP, "package.json"))("electron") as unknown;
  assert.equal(typeof binary, "string", "web/apps/desktop's electron package names its binary; run npm ci there");
  assert.ok(path.isAbsolute(binary as string), "the electron binary path is absolute");
  return binary as string;
}

function electronEnv(dir: string, launch: DesktopLaunch): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [name, value] of Object.entries(process.env)) {
    if (value !== undefined && !(DROPPED as readonly string[]).includes(name)) env[name] = value;
  }
  Object.assign(env, { E2E_DESKTOP_DIR: dir, E2E_DESKTOP_AETHER_SOCKET: launch.aetherSocket, E2E_DESKTOP_ARCHON_SOCKET: launch.archonSocket });
  if (launch.trust !== undefined) {
    assert.match(launch.trust.fingerprint, /^sha256\//, "a certificate is trusted by its sha256 fingerprint");
    Object.assign(env, { E2E_DESKTOP_TRUST_ORIGIN: launch.trust.origin, E2E_DESKTOP_TRUST_FINGERPRINT: launch.trust.fingerprint });
  }
  return env;
}

/** Lays `page` out at desktop width, where the nav rail shows the Connections list. */
export async function widen(page: Page): Promise<void> {
  await page.setViewportSize(DESKTOP_VIEWPORT);
  const width = await page.evaluate(() => window.innerWidth);
  assert.equal(width, DESKTOP_VIEWPORT.width, "the page lays out at desktop width");
}

async function appWindow(electron: ElectronApplication): Promise<Page> {
  const window = await electron.firstWindow({ timeout: LAUNCH_MS });
  await expect.poll(() => window.url(), { message: "the first window is the app:// window", timeout: LAUNCH_MS }).toMatch(/^app:\/\//);
  await expect.poll(() => electron.evaluate(() => (globalThis as { e2e?: { desktop?: unknown } }).e2e?.desktop !== undefined),
    { message: "startDesktop finished", timeout: LAUNCH_MS }).toBe(true);
  await widen(window);
  return window;
}

/** Starts the desktop app with `--ozone-platform=headless`; its user data lives in a fresh temp dir. */
export async function launchDesktop(launch: DesktopLaunch): Promise<DesktopApp> {
  assert.ok(path.isAbsolute(launch.aetherSocket) && path.isAbsolute(launch.archonSocket), "the app gets absolute socket paths");
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await launchOnce(launch);
    } catch (error) {
      const hung = error instanceof Error && error.name === "TimeoutError" && error.message.startsWith("electron.launch");
      if (!hung || attempt >= LAUNCH_ATTEMPTS) throw error;
      process.stderr.write(`desktop flows: electron.launch attempt ${attempt} hung in Playwright's handshake; launching again\n`);
    }
  }
}

async function launchOnce(launch: DesktopLaunch): Promise<DesktopApp> {
  const dir = await mkdtemp(path.join(tempBase(), "dt-"));
  record("roots", dir);
  let electron: ElectronApplication | undefined;
  try {
    electron = await _electron.launch({
      executablePath: electronBinary(), args: ["--ozone-platform=headless", MAIN], env: electronEnv(dir, launch), cwd: dir, timeout: HANDSHAKE_MS,
    });
    const pid = electron.process().pid;
    assert.ok(pid !== undefined, "Electron started");
    record("pids", `${pid} ${startTime(pid) ?? "gone"} electron`);
    return desktopApp(electron, await appWindow(electron), dir);
  } catch (error) {
    // Closes what started, then fails with the launch's own error.
    await electron?.close().catch(() => undefined);
    await rm(dir, { recursive: true, force: true, maxRetries: 3 });
    throw error;
  }
}

type Fault = { cmd: string; code: string; message: string };
type MainState = { e2e: { calls: Array<{ method: string; args: unknown[] }>; confirmations: Shown[]; answers: boolean[]; faults: Fault[] } };

function desktopApp(electron: ElectronApplication, window: Page, dir: string): DesktopApp {
  let closed = false;
  return {
    electron, window,
    calls: () => electron.evaluate(() => structuredClone((globalThis as unknown as MainState).e2e.calls)),
    confirmations: () => electron.evaluate(() => (globalThis as unknown as MainState).e2e.confirmations.map((shown) => ({ ...shown }))),
    answer: async (...answers) => {
      await electron.evaluate((_electron, queued) => { (globalThis as unknown as MainState).e2e.answers.push(...queued); }, answers);
    },
    refuseNext: async (cmd, message) => {
      assert.ok(cmd !== "" && message !== "", "a scripted refusal names its command and its reason");
      await electron.evaluate((_electron, fault) => { (globalThis as unknown as MainState).e2e.faults.push(fault); }, { cmd, code: "invalid_request", message });
    },
    async close() {
      if (closed) return;
      closed = true;
      try {
        await electron.close();
      } finally {
        await rm(dir, { recursive: true, force: true, maxRetries: 3 });
      }
    },
  };
}
