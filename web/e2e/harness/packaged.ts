// The packaged desktop app (target/desktop/linux-unpacked/aether-desktop) on a stack's sockets,
// under a private headless sway. Its fuses turn off Node's inspector, so `_electron.launch`
// cannot attach; Playwright drives the app window over Chromium's DevTools port instead.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { access, constants, mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
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
  /** The X display of the Xwayland sway hosts, when it was asked for one; the app then runs on X11. */
  readonly x11Display?: string;
  stop(): Promise<void>;
}

export interface SwayOptions {
  /**
   * Host an Xwayland too, and run the app on it. The seat's virtual keyboard never reaches a
   * GTK dialog of a Wayland app, but xdotool's presses reach one on X11.
   */
  readonly xwayland?: boolean;
}

/** The Xwayland that sway started in `runtime`: the one Xwayland process that inherited that dir. */
async function xwaylandDisplay(runtime: string): Promise<string | undefined> {
  const { stdout } = await run("/usr/bin/pgrep", ["-u", String(process.getuid?.() ?? ""), "-x", "Xwayland"], { timeout: TOOL_MS }).catch(() => ({ stdout: "" }));
  for (const pid of stdout.split("\n").filter((line) => /^\d+$/.test(line))) {
    const environ = await readFile(`/proc/${pid}/environ`, "utf8").catch(() => "");
    if (!environ.split("\0").includes(`XDG_RUNTIME_DIR=${runtime}`)) continue;
    const display = (await readFile(`/proc/${pid}/cmdline`, "utf8").catch(() => "")).split("\0")[1];
    assert.ok(display !== undefined && /^:\d+$/.test(display), `Xwayland ${pid} names its display first: ${display}`);
    return display;
  }
  return undefined;
}

async function swayIpc(runtime: string): Promise<string | undefined> {
  const names = await readdir(runtime).catch(() => [] as string[]);
  const socket = names.find((name) => name.startsWith("sway-ipc.") && name.endsWith(".sock"));
  return socket === undefined ? undefined : path.join(runtime, socket);
}

/**
 * `WLR_BACKENDS=headless sway` in a runtime dir of its own under `root`, on an empty config, or
 * on one that starts Xwayland at once when the app is to run on X11.
 */
export async function startSway(root: string, options: SwayOptions = {}): Promise<Sway> {
  assert.ok(path.isAbsolute(root), `sway's root is absolute: ${root}`);
  const runtime = path.join(root, "run");
  await mkdir(runtime, { recursive: true, mode: 0o700 });
  const xwayland = options.xwayland === true;
  if (xwayland) await access(XDOTOOL, constants.X_OK).catch((error: unknown) => { throw new Error(`the app on X11 needs ${XDOTOOL} for its native dialogs`, { cause: error }); });
  const config = xwayland ? path.join(root, "sway.conf") : "/dev/null";
  if (xwayland) await writeFile(config, "xwayland force\n");
  const env = { PATH: SYSTEM_PATH, HOME: root, XDG_RUNTIME_DIR: runtime, WLR_BACKENDS: "headless", WLR_LIBINPUT_NO_DEVICES: "1", WLR_RENDERER: "pixman" };
  const sway = spawnLogged("/usr/bin/sway", ["-c", config], { env, cwd: root, log: path.join(root, "sway.log") });
  let keyboard: Spawned | undefined;
  const stop = async () => { if (keyboard !== undefined) await stopGroup(keyboard); await stopGroup(sway); };
  try {
    const display = path.join(runtime, "wayland-1");
    await until(async () => sway.child.exitCode === null && await access(display).then(() => true, () => false), Boolean, "sway's Wayland socket", START_MS);
    const ipc = await until(() => swayIpc(runtime), (found) => found !== undefined, "sway's IPC socket", START_MS);
    assert.ok(ipc !== undefined, "sway opened its IPC socket");
    // A headless seat has no keyboard until a virtual one exists; this one stays, so every
    // client sees a keyboard from its first frame and the key presses later reach its dialogs.
    keyboard = spawnLogged("/usr/bin/wtype", ["-s", String(KEYBOARD_MS)], { env: { ...env, WAYLAND_DISPLAY: "wayland-1" }, cwd: root, log: path.join(root, "keyboard.log") });
    if (!xwayland) return { display, runtime, ipc, stop };
    const x11Display = await until(() => xwaylandDisplay(runtime), (found) => found !== undefined, "sway's Xwayland display", START_MS);
    assert.ok(x11Display !== undefined, "sway started its Xwayland");
    return { display, runtime, ipc, x11Display, stop };
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

/** Only these reach the app: its own home and XDG dirs, one display and the two sockets. */
function appEnv(dir: string, launch: PackagedLaunch): NodeJS.ProcessEnv {
  const { sway } = launch;
  const display: NodeJS.ProcessEnv = sway.x11Display === undefined
    ? { WAYLAND_DISPLAY: sway.display, XDG_SESSION_TYPE: "wayland" }
    : { DISPLAY: sway.x11Display, XDG_SESSION_TYPE: "x11" };
  const env: NodeJS.ProcessEnv = {
    PATH: SYSTEM_PATH, HOME: path.join(dir, "home"),
    XDG_CONFIG_HOME: path.join(dir, "config"), XDG_CACHE_HOME: path.join(dir, "cache"), XDG_DATA_HOME: path.join(dir, "data"),
    XDG_RUNTIME_DIR: sway.runtime, ...display,
    AETHER_SOCKET: launch.aetherSocket, ARCHON_SOCKET: launch.archonSocket,
  };
  for (const name of ["LANG", "LC_ALL", "TZ"]) if (process.env[name] !== undefined) env[name] = process.env[name];
  return env;
}

let running: PackagedApp | undefined;
/** On a host under heavy load the app can take the DevTools socket yet not answer in time. */
const LAUNCH_ATTEMPTS = 2;

/**
 * Starts the packaged app, connects over its DevTools port and returns its app window. One at a
 * time. A start that does not answer is stopped and tried once more; both failures are kept.
 */
export async function launchPackaged(launch: PackagedLaunch): Promise<PackagedApp> {
  const failures: unknown[] = [];
  for (let attempt = 1; attempt <= LAUNCH_ATTEMPTS; attempt += 1) {
    try {
      return await launchOnce(launch);
    } catch (error) {
      failures.push(error);
      console.warn(`the packaged app did not start (attempt ${attempt} of ${LAUNCH_ATTEMPTS}): ${firstLine(error)}; cause: ${firstLine(error instanceof Error ? error.cause : undefined)}`);
    }
  }
  assert.equal(failures.length, LAUNCH_ATTEMPTS, "every attempt failed");
  throw new AggregateError(failures, `the packaged app did not start in ${LAUNCH_ATTEMPTS} attempts`);
}

async function launchOnce(launch: PackagedLaunch): Promise<PackagedApp> {
  assert.equal(running, undefined, "one packaged app runs at a time; close the last one first");
  assert.ok(path.isAbsolute(launch.aetherSocket) && path.isAbsolute(launch.archonSocket), "the app gets absolute socket paths");
  const dir = await mkdtemp(path.join(tempBase(), "pk-"));
  record("roots", dir);
  for (const sub of ["home", "config", "cache", "data"]) await mkdir(path.join(dir, sub), { mode: 0o700 });
  const log = path.join(dir, "app.log");
  let app: Spawned | undefined;
  let browser: Browser | undefined;
  try {
    const platform = launch.sway.x11Display === undefined ? "wayland" : "x11";
    app = spawnLogged(launch.exec, [`--ozone-platform=${platform}`, "--remote-debugging-port=0", "--remote-debugging-address=127.0.0.1"],
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

interface SwayWindow { readonly title: string; readonly focused: boolean }

/** Every window sway shows, native dialogs included, and whether it has the keyboard. */
async function swayWindows(sway: Sway): Promise<SwayWindow[]> {
  const { stdout } = await run("/usr/bin/swaymsg", ["-t", "get_tree", "-r"], { env: swayEnv(sway), timeout: TOOL_MS });
  const windows: SwayWindow[] = [];
  const walk = (node: { name?: unknown; focused?: unknown; nodes?: unknown[]; floating_nodes?: unknown[] }) => {
    const children = [...(node.nodes ?? []), ...(node.floating_nodes ?? [])];
    if (typeof node.name === "string" && children.length === 0) windows.push({ title: node.name, focused: node.focused === true });
    for (const child of children) walk(child as typeof node);
  };
  walk(JSON.parse(stdout) as Parameters<typeof walk>[0]);
  assert.ok(windows.filter((window) => window.focused).length <= 1, "sway gives the keyboard to one window at most");
  return windows;
}

/** Titles of every window sway shows, native dialogs included. */
export async function windowTitles(sway: Sway): Promise<string[]> {
  return (await swayWindows(sway)).map((window) => window.title);
}

const firstLine = (error: unknown): string => (error instanceof Error ? error.message : String(error)).split("\n")[0] ?? "";

const DIALOG_CLOSE_MS = 3_000;

/** Whether the window titled `title` is gone within the bound; a failing swaymsg still throws. */
async function closesWithin(sway: Sway, title: string, timeoutMs: number): Promise<boolean> {
  assert.ok(timeoutMs > 0, "a close is awaited for a positive time");
  const end = Date.now() + timeoutMs;
  while (Date.now() < end) {
    if (!(await windowTitles(sway)).includes(title)) return true;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return !(await windowTitles(sway)).includes(title);
}

/**
 * Key presses that reach a confirmation's action button, by toolkit. GTK 3 focuses Cancel, so
 * Right moves to the action; GTK 4 opens with nothing focused, so Tab lands on Cancel and a
 * second Tab on the action; a GTK 4 dialog that did focus Cancel needs one Tab.
 */
export const DIALOG_PRESSES: ReadonlyArray<readonly string[]> = [["Right", "Return"], ["Tab", "Tab", "Return"], ["Tab", "Return"]];
const KEY_GAP_MS = "150";
const XDOTOOL = "/usr/bin/xdotool";
/** How long the seat's standing virtual keyboard sleeps: longer than any journey. */
const KEYBOARD_MS = 6 * 60 * 60 * 1000;

async function focusDialog(sway: Sway, title: string, timeoutMs: number): Promise<void> {
  const env = swayEnv(sway);
  const criteria = `[title="^${title.replace(/[.*+?^${}()|[\]\\"]/g, "\\$&")}$"]`;
  await run("/usr/bin/swaymsg", [criteria, "focus"], { env, timeout: TOOL_MS });
  const windows = await until(() => swayWindows(sway), (now) => now.some((window) => window.title === title && window.focused),
    `the native dialog "${title}" to take the keyboard`, timeoutMs);
  assert.ok(windows.some((window) => window.title === title && window.focused), "the dialog has the keyboard");
}

/**
 * Closes the native confirmation titled `title` (security/confirm.ts) with the key presses of
 * DIALOG_PRESSES from `from` on, each tried once the dialog has the keyboard, until one closes
 * it. Returns that one's index: a press can also land on Cancel, so the caller checks the
 * effect and asks again from the next index when the action did not happen.
 */
export async function acceptNativeDialog(sway: Sway, title: string, from = 0, timeoutMs = 15_000): Promise<number> {
  assert.ok(title.trim() !== "", "a native dialog is found by its title");
  assert.ok(from >= 0 && from < DIALOG_PRESSES.length, `presses start within the list: ${from}`);
  await until(() => windowTitles(sway), (titles) => titles.includes(title), `the native dialog "${title}"`, timeoutMs);
  for (let index = from; index < DIALOG_PRESSES.length; index += 1) {
    const presses = DIALOG_PRESSES[index] as readonly string[];
    await focusDialog(sway, title, timeoutMs);
    await press(sway, presses);
    if (await closesWithin(sway, title, DIALOG_CLOSE_MS)) return index;
    console.warn(`the native dialog "${title}" stayed open after ${presses.join("+")}`);
  }
  const focused = (await swayWindows(sway)).find((window) => window.focused)?.title ?? "nothing";
  const evidence = await dialogEvidence(sway, title);
  throw new Error(`the native dialog "${title}" did not close after ${DIALOG_PRESSES.length} key sequences; sway's keyboard is on ${focused}; the app maps ${await toolkitOf(running?.pid)}; see ${evidence}`);
}

/** `keys` to the focused window: through the seat's virtual keyboard, or xdotool when the app runs on X11. */
async function press(sway: Sway, keys: readonly string[]): Promise<void> {
  assert.ok(keys.length > 0 && keys.every((key) => /^[A-Za-z]+$/.test(key)), `key names only: ${keys.join(" ")}`);
  if (sway.x11Display === undefined) {
    await run("/usr/bin/wtype", keys.flatMap((key) => ["-s", KEY_GAP_MS, "-k", key]), { env: swayEnv(sway), timeout: TOOL_MS });
    return;
  }
  await run(XDOTOOL, ["key", "--delay", KEY_GAP_MS, ...keys], { env: { ...swayEnv(sway), DISPLAY: sway.x11Display }, timeout: TOOL_MS });
}

/** The GTK libraries the app process maps: which toolkit drew the dialog. */
async function toolkitOf(pid: number | undefined): Promise<string> {
  if (pid === undefined) return "no app";
  const maps = await readFile(`/proc/${pid}/maps`, "utf8").catch(() => "");
  return [...new Set(maps.match(/libgtk-\d[^ /]*/g) ?? [])].join(" ") || "no GTK";
}

/** A screenshot of sway's output and its window tree, kept under the run's output for a dialog that would not close. */
async function dialogEvidence(sway: Sway, title: string): Promise<string> {
  const dir = path.join(process.env.E2E_OUT ?? tempBase(), "journeys", "dialogs");
  await mkdir(dir, { recursive: true });
  const stem = path.join(dir, `${title.toLowerCase().replace(/[^a-z0-9]+/g, "-")}-${Date.now()}`);
  const env = swayEnv(sway);
  await run("/usr/bin/grim", [`${stem}.png`], { env, timeout: TOOL_MS });
  const { stdout } = await run("/usr/bin/swaymsg", ["-t", "get_tree", "-r"], { env, timeout: TOOL_MS });
  await writeFile(`${stem}.json`, stdout);
  return `${stem}.png`;
}
