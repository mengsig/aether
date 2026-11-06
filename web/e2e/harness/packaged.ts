// The packaged desktop app (target/desktop/linux-unpacked/aether-desktop) on a stack's sockets,
// under a private headless sway. Its fuses turn off Node's inspector, so `_electron.launch`
// cannot attach; Playwright drives the app window over Chromium's DevTools port instead.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { access, constants, mkdir, mkdtemp, readdir, readFile, rm } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { chromium, expect, type Browser, type Page } from "@playwright/test";
import { tempBase } from "./env.ts";
import { record, spawnLogged, stopGroup, tail, until, type Spawned } from "./process.ts";

const run = promisify(execFile);
const REPO_ROOT = path.resolve(import.meta.dirname, "..", "..", "..");
export const PACKAGED_EXEC = path.join(REPO_ROOT, "target", "desktop", "linux-unpacked", "aether-desktop");
const SYSTEM_PATH = "/usr/local/bin:/usr/bin:/bin";
const START_MS = 30_000;
const LAUNCH_MS = 60_000;
const TOOL_MS = 10_000;
/** The app window's own size (app-window.ts). */
const DESKTOP_WIDTH = 1280;
const DEVTOOLS_LINE = /DevTools listening on (ws:\/\/127\.0\.0\.1:\d+\/devtools\/browser\/[\w-]+)/;

/** `E2E_DESKTOP_EXEC`, else this checkout's packaged build; either must be an executable. */
export async function packagedExec(env: NodeJS.ProcessEnv = process.env): Promise<string> {
  const exec = env.E2E_DESKTOP_EXEC ?? PACKAGED_EXEC;
  assert.ok(path.isAbsolute(exec), `the packaged app path is absolute: ${exec}`);
  await access(exec, constants.X_OK).catch((error: unknown) => {
    throw new Error(`no packaged app at ${exec}; run scripts/e2e_journeys.sh, which packages it`, { cause: error });
  });
  return exec;
}

export interface Sway {
  /** The compositor's Wayland socket, by absolute path. */
  readonly display: string;
  readonly runtime: string;
  /** sway's IPC socket, for swaymsg. */
  readonly ipc: string;
  stop(): Promise<void>;
}

async function swayIpc(runtime: string): Promise<string | undefined> {
  const names = await readdir(runtime).catch(() => [] as string[]);
  const socket = names.find((name) => name.startsWith("sway-ipc.") && name.endsWith(".sock"));
  return socket === undefined ? undefined : path.join(runtime, socket);
}

/** `WLR_BACKENDS=headless sway -c /dev/null` in a runtime dir of its own under `root`. */
export async function startSway(root: string): Promise<Sway> {
  assert.ok(path.isAbsolute(root), `sway's root is absolute: ${root}`);
  const runtime = path.join(root, "run");
  await mkdir(runtime, { recursive: true, mode: 0o700 });
  const env = { PATH: SYSTEM_PATH, HOME: root, XDG_RUNTIME_DIR: runtime, WLR_BACKENDS: "headless", WLR_LIBINPUT_NO_DEVICES: "1", WLR_RENDERER: "pixman" };
  const sway = spawnLogged("/usr/bin/sway", ["-c", "/dev/null"], { env, cwd: root, log: path.join(root, "sway.log") });
  const stop = () => stopGroup(sway);
  try {
    const display = path.join(runtime, "wayland-1");
    await until(async () => sway.child.exitCode === null && await access(display).then(() => true, () => false), Boolean, "sway's Wayland socket", START_MS);
    const ipc = await until(() => swayIpc(runtime), (found) => found !== undefined, "sway's IPC socket", START_MS);
    assert.ok(ipc !== undefined, "sway opened its IPC socket");
    return { display, runtime, ipc, stop };
  } catch (error) {
    await stop();
    throw new Error(`sway did not start. Its log:\n${await tail(path.join(root, "sway.log"))}`, { cause: error });
  }
}

export interface PackagedLaunch {
  readonly exec: string;
  readonly sway: Sway;
  readonly aetherSocket: string;
  readonly archonSocket: string;
}

export interface PackagedApp {
  readonly browser: Browser;
  /** The app:// window, which reaches the daemons only through window.desktop. */
  readonly window: Page;
  readonly pid: number;
  readonly log: string;
  close(): Promise<void>;
}

/** Only these reach the app: its own home and XDG dirs, the display and the two sockets. */
function appEnv(dir: string, launch: PackagedLaunch): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {
    PATH: SYSTEM_PATH, HOME: path.join(dir, "home"),
    XDG_CONFIG_HOME: path.join(dir, "config"), XDG_CACHE_HOME: path.join(dir, "cache"), XDG_DATA_HOME: path.join(dir, "data"),
    XDG_RUNTIME_DIR: launch.sway.runtime, WAYLAND_DISPLAY: launch.sway.display, XDG_SESSION_TYPE: "wayland",
    AETHER_SOCKET: launch.aetherSocket, ARCHON_SOCKET: launch.archonSocket,
  };
  for (const name of ["LANG", "LC_ALL", "TZ"]) if (process.env[name] !== undefined) env[name] = process.env[name];
  return env;
}

let running: PackagedApp | undefined;

/** Starts the packaged app, connects over its DevTools port and returns its app window. One at a time. */
export async function launchPackaged(launch: PackagedLaunch): Promise<PackagedApp> {
  assert.equal(running, undefined, "one packaged app runs at a time; close the last one first");
  assert.ok(path.isAbsolute(launch.aetherSocket) && path.isAbsolute(launch.archonSocket), "the app gets absolute socket paths");
  const dir = await mkdtemp(path.join(tempBase(), "pk-"));
  record("roots", dir);
  for (const sub of ["home", "config", "cache", "data"]) await mkdir(path.join(dir, sub), { mode: 0o700 });
  const log = path.join(dir, "app.log");
  let app: Spawned | undefined;
  let browser: Browser | undefined;
  try {
    app = spawnLogged(launch.exec, ["--ozone-platform=wayland", "--remote-debugging-port=0", "--remote-debugging-address=127.0.0.1"],
      { env: appEnv(dir, launch), cwd: dir, log });
    const started = app;
    const endpoint = await until(async () => DEVTOOLS_LINE.exec(await readFile(log, "utf8").catch(() => ""))?.[1],
      (found) => found !== undefined || started.child.exitCode !== null, "the app's DevTools endpoint", LAUNCH_MS);
    assert.ok(endpoint !== undefined, `the app exited before it listened (status ${String(started.child.exitCode)})`);
    browser = await chromium.connectOverCDP(endpoint, { timeout: LAUNCH_MS });
    const window = await appWindow(browser);
    running = packagedApp(browser, window, started, dir);
    return running;
  } catch (error) {
    await browser?.close();
    if (app !== undefined) await stopGroup(app);
    const output = await tail(log);
    await rm(dir, { recursive: true, force: true, maxRetries: 3 });
    throw new Error(`the packaged app did not start. Its log:\n${output}`, { cause: error });
  }
}

async function appWindow(browser: Browser): Promise<Page> {
  const pages = () => browser.contexts().flatMap((context) => context.pages());
  await expect.poll(() => pages().map((page) => page.url()), { message: "the app:// window opened", timeout: LAUNCH_MS })
    .toContainEqual(expect.stringMatching(/^app:\/\//));
  const app = pages().find((page) => page.url().startsWith("app://"));
  assert.ok(app !== undefined, "the app window is among the pages");
  await app.waitForLoadState("domcontentloaded");
  await expect.poll(() => app.evaluate(() => typeof (window as unknown as { desktop?: unknown }).desktop), { message: "window.desktop is there" })
    .toBe("object");
  const width = await app.evaluate(() => window.innerWidth);
  assert.ok(width >= DESKTOP_WIDTH - 40, `the app window lays out at desktop width, not ${width}`);
  return app;
}

function packagedApp(browser: Browser, window: Page, app: Spawned, dir: string): PackagedApp {
  let closed = false;
  return {
    browser, window, pid: app.pid, log: app.log,
    async close() {
      if (closed) return;
      closed = true;
      running = undefined;
      // Every step runs even when one fails, so a stuck app leaks nothing else.
      const failures: unknown[] = [];
      for (const step of [() => browser.close(), () => stopGroup(app), () => rm(dir, { recursive: true, force: true, maxRetries: 3 })]) {
        await step().catch((error: unknown) => { failures.push(error); });
      }
      if (failures.length > 0) throw new AggregateError(failures, `the packaged app (pid ${app.pid}) did not close cleanly`);
    },
  };
}

function swayEnv(sway: Sway): NodeJS.ProcessEnv {
  return { PATH: SYSTEM_PATH, XDG_RUNTIME_DIR: sway.runtime, WAYLAND_DISPLAY: sway.display, SWAYSOCK: sway.ipc };
}

/** Titles of every window sway shows, native dialogs included. */
export async function windowTitles(sway: Sway): Promise<string[]> {
  const { stdout } = await run("/usr/bin/swaymsg", ["-t", "get_tree", "-r"], { env: swayEnv(sway), timeout: TOOL_MS });
  const titles: string[] = [];
  const walk = (node: { name?: unknown; nodes?: unknown[]; floating_nodes?: unknown[] }) => {
    if (typeof node.name === "string" && (node.nodes?.length ?? 0) === 0) titles.push(node.name);
    for (const child of [...(node.nodes ?? []), ...(node.floating_nodes ?? [])]) walk(child as typeof node);
  };
  walk(JSON.parse(stdout) as Parameters<typeof walk>[0]);
  return titles;
}

/**
 * Accepts the native confirmation titled `title` (security/confirm.ts): its buttons are Cancel
 * then the action, Cancel focused, so Right moves to the action and Return presses it.
 */
export async function acceptNativeDialog(sway: Sway, title: string, timeoutMs = 15_000): Promise<void> {
  assert.ok(title.trim() !== "", "a native dialog is found by its title");
  await until(() => windowTitles(sway), (titles) => titles.includes(title), `the native dialog "${title}"`, timeoutMs);
  const env = swayEnv(sway);
  await run("/usr/bin/swaymsg", [`[title="^${title.replace(/[.*+?^${}()|[\]\\"]/g, "\\$&")}$"]`, "focus"], { env, timeout: TOOL_MS });
  await run("/usr/bin/wtype", ["-k", "Right", "-k", "Return"], { env, timeout: TOOL_MS });
  await until(() => windowTitles(sway), (titles) => !titles.includes(title), `the native dialog "${title}" to close`, timeoutMs);
}
