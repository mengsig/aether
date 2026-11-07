// The packaged desktop app, for a flow that must prove the shipped build: E2E_DESKTOP_PACKAGED
// names its binary. Its fuses close the Node inspector, so Playwright drives it over CDP.
// The shipped main shows a real window, so it runs on a private headless sway, never the desktop.

import assert from "node:assert/strict";
import { accessSync, constants, existsSync, readdirSync, readFileSync } from "node:fs";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { chromium, expect, type Browser, type Page } from "@playwright/test";
import { tempBase } from "../../../harness/env.ts";
import { record, spawnLogged, stopGroup, type Spawned } from "../../../harness/process.ts";
import { failLaunch, runAll } from "../../../harness/teardown.ts";

const READY_MS = 60_000;
const POLL_MS = 200;
/** sway's IPC socket, `sway-ipc.<uid>.<pid>.sock`, must fit in a socket address (108 bytes). */
const MAX_RUNTIME_DIR = 70;
const SWAY_CONFIG = "output HEADLESS-1 resolution 1600x1000\nxwayland disable\n";

export interface PackagedApp {
  readonly window: Page;
  readonly waylandDisplay: string;
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

/** `name` on PATH, as spawnLogged wants it: absolute. */
function onPath(name: string): string {
  const dirs = (process.env.PATH ?? "").split(path.delimiter).filter((entry) => path.isAbsolute(entry));
  const at = dirs.map((dir) => path.join(dir, name)).find((candidate) => existsSync(candidate));
  if (at === undefined) throw new Error(`the packaged flow runs the app on a headless ${name}, which is not on PATH`);
  accessSync(at, constants.X_OK);
  return at;
}

async function until<T>(read: () => T | undefined, what: string, alive: Spawned): Promise<T> {
  const deadline = Date.now() + READY_MS;
  for (;;) {
    const value = read();
    if (value !== undefined) return value;
    assert.ok(alive.child.exitCode === null && alive.child.signalCode === null, `${path.basename(alive.log)}: it exited before ${what}`);
    assert.ok(Date.now() < deadline, `timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  }
}

/** Starts a headless sway whose Wayland socket will appear in `runtime`. */
async function spawnCompositor(root: string, runtime: string, env: NodeJS.ProcessEnv): Promise<Spawned> {
  assert.ok(runtime.length <= MAX_RUNTIME_DIR, `the compositor's runtime dir is short enough for its sockets: ${runtime}`);
  await mkdir(runtime, { mode: 0o700 });
  const config = path.join(root, "sway.conf");
  await writeFile(config, SWAY_CONFIG);
  const swayEnv: NodeJS.ProcessEnv = { ...env, XDG_RUNTIME_DIR: runtime, WLR_BACKENDS: "headless", WLR_RENDERER: "pixman", WLR_LIBINPUT_NO_DEVICES: "1" };
  return spawnLogged(onPath("sway"), ["-c", config], { env: swayEnv, cwd: root, log: path.join(root, "sway.log") });
}

/** The absolute path of the Wayland socket `sway` opens in `runtime`. */
async function waylandSocket(runtime: string, sway: Spawned): Promise<string> {
  const socket = await until(() => readdirSync(runtime).find((name) => /^wayland-\d+$/.test(name)), "sway's Wayland socket", sway);
  return path.join(runtime, socket);
}

async function devtoolsPort(profile: string, app: Spawned): Promise<number> {
  const file = path.join(profile, "DevToolsActivePort");
  await until(() => (existsSync(file) ? true : undefined), "the packaged app's DevTools port", app);
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

/** The end of a log, for a failed launch's message; the root that holds it is then removed. */
function logTail(log: string): string {
  if (!existsSync(log)) return "(no log was written)";
  return readFileSync(log, "utf8").split("\n").slice(-40).join("\n");
}

/** Starts the packaged app on `sockets`, its profile, compositor and logs in a fresh root under the run's temp dir. */
export async function launchPackaged(binary: string, sockets: { aether: string; archon: string }): Promise<PackagedApp> {
  assert.ok(path.isAbsolute(sockets.aether) && path.isAbsolute(sockets.archon), "the app gets absolute socket paths");
  const root = await mkdtemp(path.join(tempBase(), "pk-"));
  record("roots", root);
  const profile = path.join(root, "profile");
  const log = path.join(root, "app.log");
  const env: NodeJS.ProcessEnv = { ...process.env, AETHER_SOCKET: sockets.aether, ARCHON_SOCKET: sockets.archon };
  delete env.DISPLAY;
  delete env.WAYLAND_DISPLAY;
  delete env.SWAYSOCK;
  let sway: Spawned | undefined;
  let app: Spawned | undefined;
  let browser: Browser | undefined;
  const close = () => runAll([
    async () => browser?.close(),
    async () => app !== undefined && stopGroup(app),
    async () => sway !== undefined && stopGroup(sway),
    () => rm(root, { recursive: true, force: true, maxRetries: 3 }),
  ]);
  try {
    const runtime = path.join(root, "run");
    sway = await spawnCompositor(root, runtime, env);
    const display = await waylandSocket(runtime, sway);
    const appEnv = { ...env, XDG_RUNTIME_DIR: runtime, WAYLAND_DISPLAY: display };
    app = spawnLogged(binary, [`--user-data-dir=${profile}`, "--ozone-platform=wayland", "--remote-debugging-address=127.0.0.1", "--remote-debugging-port=0"],
      { env: appEnv, cwd: root, log });
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${await devtoolsPort(profile, app)}`);
    return { window: await appWindow(browser), waylandDisplay: display, close };
  } catch (error) {
    const tails = `${logTail(log)}\n-- sway:\n${logTail(path.join(root, "sway.log"))}`;
    return failLaunch(error, `The packaged app did not start; its logs ended:\n${tails}`, close);
  }
}
