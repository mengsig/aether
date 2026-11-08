// The packaged desktop app, for a flow that must prove the shipped build: E2E_DESKTOP_PACKAGED
// names its binary. Its fuses close the Node inspector, so Playwright drives it over CDP.
// The shipped main shows a real window, so it runs on a private headless sway, never the desktop,
// with a private session bus and a recording `xdg-open`: no notification or browser escapes.

import assert from "node:assert/strict";
import { accessSync, constants, existsSync, readdirSync, readFileSync } from "node:fs";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { chromium, expect, type Browser, type Page } from "@playwright/test";
import { tempBase } from "../../../harness/env.ts";
import { record, spawnLogged, stopGroup, type Spawned } from "../../../harness/process.ts";
import { failLaunch, runAll } from "../../../harness/teardown.ts";
import { startNativeHost, type NativeHost } from "./native-host.ts";

const READY_MS = 60_000;
const POLL_MS = 200;
/** sway's IPC socket, `sway-ipc.<uid>.<pid>.sock`, must fit in a socket address (108 bytes). */
const MAX_RUNTIME_DIR = 70;
const SWAY_CONFIG = "output HEADLESS-1 resolution 1600x1000\nxwayland disable\n";
/** How long a second instance may take to hand its link over and exit. */
const AGAIN_MS = 30_000;

export interface PackagedApp {
  readonly window: Page;
  /** Every page the app has open, the app window and any report window among them. */
  readonly browser: Browser;
  /** The run's own notification server and `xdg-open`. */
  readonly native: NativeHost;
  /** The environment that reaches the run's own compositor, for its keyboard and window list. */
  readonly compositor: NodeJS.ProcessEnv;
  /** Runs the binary again on the same profile, as the system does for a link; resolves once it exits. */
  again(args: readonly string[]): Promise<number | null>;
  close(): Promise<void>;
}

export interface PackagedOptions {
  /** Extra command-line arguments, such as a launch-time `aether-app://` link. */
  readonly args?: readonly string[];
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

/**
 * Waits until the app window has committed its own document. Attaching while it is still on the
 * initial empty one races that first navigation, whose commit Playwright then never sees.
 */
async function appDocument(port: number, app: Spawned): Promise<void> {
  const deadline = Date.now() + READY_MS;
  for (;;) {
    assert.ok(app.child.exitCode === null && app.child.signalCode === null, `${path.basename(app.log)}: it exited before its window loaded`);
    assert.ok(Date.now() < deadline, "timed out waiting for the app window's document");
    if (await hasAppDocument(port, deadline)) return;
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  }
}

/** Whether DevTools lists an app:// page with its own title; a stalled answer fails at `deadline`. */
async function hasAppDocument(port: number, deadline: number): Promise<boolean> {
  const signal = AbortSignal.timeout(Math.max(1, deadline - Date.now()));
  let targets: unknown;
  try {
    const reply = await fetch(`http://127.0.0.1:${port}/json/list`, { signal });
    assert.ok(reply.ok, `DevTools lists the app's targets: HTTP ${reply.status}`);
    targets = await reply.json();
  } catch (error) {
    throw new Error(`DevTools on port ${port} gave no target list`, { cause: error });
  }
  assert.ok(Array.isArray(targets), "DevTools answers a target list");
  return (targets as Array<{ type?: unknown; url?: unknown; title?: unknown }>).some((target) =>
    target.type === "page" && String(target.url).startsWith("app://") && /(^|· )Aether$/.test(String(target.title)));
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

/** sway's IPC socket in `runtime`, for swaymsg. */
function swaySocket(runtime: string): string | undefined {
  const name = readdirSync(runtime).find((entry) => /^sway-ipc\..+\.sock$/.test(entry));
  return name === undefined ? undefined : path.join(runtime, name);
}

/** Runs a second instance of `binary` with `args`, which hands them to the first and exits. */
async function runAgain(binary: string, args: readonly string[], options: { env: NodeJS.ProcessEnv; cwd: string; log: string }): Promise<number | null> {
  assert.equal(args.filter((arg) => arg.startsWith("--user-data-dir=")).length, 1, "a second instance names exactly one profile");
  const second = spawnLogged(binary, args, options);
  const deadline = new Promise<"late">((resolve) => setTimeout(() => resolve("late"), AGAIN_MS));
  const exited = new Promise<number | null>((resolve) => second.child.once("exit", (code) => resolve(code)));
  const outcome = await Promise.race([exited, deadline]);
  if (outcome !== "late") return outcome;
  await stopGroup(second);
  throw new Error(`a second instance did not exit within ${AGAIN_MS} ms:\n${logTail(options.log)}`);
}

/** Starts the packaged app on `sockets`, its profile, compositor and logs in a fresh root under the run's temp dir. */
export async function launchPackaged(binary: string, sockets: { aether: string; archon: string }, options: PackagedOptions = {}): Promise<PackagedApp> {
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
  let native: NativeHost | undefined;
  const close = () => runAll([
    async () => browser?.close(),
    async () => app !== undefined && stopGroup(app),
    async () => native?.close(),
    async () => sway !== undefined && stopGroup(sway),
    () => rm(root, { recursive: true, force: true, maxRetries: 3 }),
  ]);
  try {
    const runtime = path.join(root, "run");
    sway = await spawnCompositor(root, runtime, env);
    const display = await waylandSocket(runtime, sway);
    native = await startNativeHost(root, env);
    const appEnv = { ...env, ...native.env, XDG_RUNTIME_DIR: runtime, WAYLAND_DISPLAY: display };
    const shared = [`--user-data-dir=${profile}`, "--ozone-platform=wayland"];
    app = spawnLogged(binary, [...shared, "--remote-debugging-address=127.0.0.1", "--remote-debugging-port=0", ...(options.args ?? [])],
      { env: appEnv, cwd: root, log });
    const port = await devtoolsPort(profile, app);
    await appDocument(port, app);
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { noDefaults: true });
    const ipc = await until(() => swaySocket(runtime), "sway's IPC socket", sway);
    const compositor = { PATH: env.PATH, XDG_RUNTIME_DIR: runtime, WAYLAND_DISPLAY: display, SWAYSOCK: ipc };
    let runs = 0;
    const again = (args: readonly string[]) => {
      assert.ok(args.every((arg) => !arg.startsWith("--user-data-dir")), "a second instance shares the first one's profile");
      runs += 1;
      return runAgain(binary, [...shared, ...args], { env: appEnv, cwd: root, log: path.join(root, `again-${runs}.log`) });
    };
    return { window: await appWindow(browser), browser, native, compositor, again, close };
  } catch (error) {
    const tails = `${logTail(log)}\n-- sway:\n${logTail(path.join(root, "sway.log"))}`;
    return failLaunch(error, `The packaged app did not start; its logs ended:\n${tails}`, close);
  }
}
