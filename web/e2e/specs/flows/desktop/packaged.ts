// The packaged desktop app, for a flow that must prove the shipped build: E2E_DESKTOP_PACKAGED
// names its binary. Its fuses close the Node inspector, so Playwright drives it over CDP.

import assert from "node:assert/strict";
import { accessSync, constants, existsSync, readFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import path from "node:path";
import { chromium, expect, type Browser, type Page } from "@playwright/test";
import { tempBase } from "../../../harness/env.ts";
import { record, spawnLogged, stopGroup, type Spawned } from "../../../harness/process.ts";

const READY_MS = 60_000;
const POLL_MS = 200;

export interface PackagedApp {
  readonly window: Page;
  close(): Promise<void>;
}

/** The packaged binary to drive, or undefined to run the flows on test/e2e/main.ts. */
export function packagedBinary(): string | undefined {
  const binary = process.env.E2E_DESKTOP_PACKAGED;
  if (binary === undefined || binary === "") return undefined;
  assert.ok(path.isAbsolute(binary) && existsSync(binary), `E2E_DESKTOP_PACKAGED names the packaged binary: ${binary}`);
  accessSync(binary, constants.X_OK);
  return binary;
}

async function devtoolsPort(profile: string, app: Spawned): Promise<number> {
  const file = path.join(profile, "DevToolsActivePort");
  const deadline = Date.now() + READY_MS;
  while (!existsSync(file)) {
    assert.ok(app.child.exitCode === null, "the packaged app exited early");
    assert.ok(Date.now() < deadline, "the packaged app opened no DevTools port");
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  }
  const port = Number(readFileSync(file, "utf8").split("\n")[0]);
  assert.ok(Number.isInteger(port) && port > 0, `DevToolsActivePort names a port: ${port}`);
  return port;
}

async function appWindow(browser: Browser): Promise<Page> {
  let found: Page | undefined;
  await expect.poll(() => {
    found = browser.contexts().flatMap((context) => context.pages()).find((page) => page.url().startsWith("app://"));
    return found !== undefined;
  }, { message: "the packaged app opens its app:// window", timeout: READY_MS }).toBe(true);
  assert.ok(found !== undefined, "the app window was found");
  return found;
}

/** The end of the app's log, for a failed launch's message; the root that holds it is then removed. */
function logTail(log: string): string {
  if (!existsSync(log)) return "(no log was written)";
  return readFileSync(log, "utf8").split("\n").slice(-40).join("\n");
}

/** Starts the packaged app on `sockets`, its profile and logs in a fresh root under the run's temp dir. */
export async function launchPackaged(binary: string, sockets: { aether: string; archon: string }): Promise<PackagedApp> {
  assert.ok(path.isAbsolute(sockets.aether) && path.isAbsolute(sockets.archon), "the app gets absolute socket paths");
  const root = await mkdtemp(path.join(tempBase(), "pk-"));
  record("roots", root);
  const profile = path.join(root, "profile");
  const log = path.join(root, "app.log");
  const env: NodeJS.ProcessEnv = { ...process.env, AETHER_SOCKET: sockets.aether, ARCHON_SOCKET: sockets.archon };
  delete env.DISPLAY;
  delete env.WAYLAND_DISPLAY;
  let app: Spawned | undefined;
  let browser: Browser | undefined;
  const close = async () => {
    await browser?.close().catch(() => undefined);
    if (app !== undefined) await stopGroup(app);
    await rm(root, { recursive: true, force: true, maxRetries: 3 });
  };
  try {
    app = spawnLogged(binary, [`--user-data-dir=${profile}`, "--ozone-platform=headless", "--remote-debugging-address=127.0.0.1", "--remote-debugging-port=0"],
      { env, cwd: root, log });
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${await devtoolsPort(profile, app)}`);
    return { window: await appWindow(browser), close };
  } catch (error) {
    const tail = logTail(log);
    await close();
    throw new Error(`The packaged app did not start; its log ended:\n${tail}`, { cause: error });
  }
}
