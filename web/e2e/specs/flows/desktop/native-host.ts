// What the packaged app's native side reaches, kept private to one run: a session bus with a
// recording notification server, an `xdg-open` that records instead of opening a browser, and
// the keyboard of the run's own compositor for native dialogs.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { spawnLogged, stopGroup, until, type Spawned } from "../../../harness/process.ts";
import { runAll } from "../../../harness/teardown.ts";

const exec = promisify(execFile);
const SINK = path.join(import.meta.dirname, "notification-sink.py");
const READY_MS = 15_000;

/** A notification the sink was asked to show. */
export interface Note { readonly id: number; readonly title: string; readonly body: string; readonly actions: readonly string[] }

export interface NativeHost {
  /** What the app gets: the private bus and the recording `xdg-open` first on PATH. */
  readonly env: NodeJS.ProcessEnv;
  /** Every URL the app handed to `xdg-open`, oldest first. */
  opened(): string[];
  notes(): Note[];
  /** Clicks notification `id`, as a person clicking its body does. */
  click(id: number): Promise<void>;
  close(): Promise<void>;
}

function lines(file: string): string[] {
  return existsSync(file) ? readFileSync(file, "utf8").split("\n").filter((line) => line !== "") : [];
}

/** A shell `xdg-open` that appends its one argument to `log` and opens nothing. */
async function recordingOpener(bin: string, log: string): Promise<void> {
  await mkdir(bin, { recursive: true, mode: 0o700 });
  await writeFile(log, "");
  const script = `#!/bin/sh\n[ "$#" -eq 1 ] || exit 2\nprintf '%s\\n' "$1" >> '${log}'\n`;
  assert.ok(!log.includes("'"), "the log path needs no quoting");
  await writeFile(path.join(bin, "xdg-open"), script, { mode: 0o755 });
}

/** A session bus that only routes: no service directories, so it never starts a real portal or notifier. */
function busConfig(socket: string): string {
  assert.ok(path.isAbsolute(socket) && !/[<>&"]/.test(socket), "the bus socket is a plain absolute path");
  return `<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=${socket}</listen>
  <policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy>
</busconfig>
`;
}

/** Starts the bus and the sink in `root`; `close` stops both. */
export async function startNativeHost(root: string, env: NodeJS.ProcessEnv): Promise<NativeHost> {
  assert.ok(path.isAbsolute(root) && existsSync(root), `the native host lives in the run's root: ${root}`);
  const socket = path.join(root, "bus");
  const address = `unix:path=${socket}`;
  const notesLog = path.join(root, "notes.jsonl");
  const openedLog = path.join(root, "opened.log");
  const bin = path.join(root, "native-bin");
  await recordingOpener(bin, openedLog);
  let bus: Spawned | undefined;
  let sink: Spawned | undefined;
  const close = () => runAll([async () => sink !== undefined && stopGroup(sink), async () => bus !== undefined && stopGroup(bus)]);
  try {
    const config = path.join(root, "bus.conf");
    await writeFile(config, busConfig(socket));
    bus = spawnLogged("/usr/bin/dbus-daemon", [`--config-file=${config}`, "--nofork", "--nopidfile"], { env, cwd: root, log: path.join(root, "bus.log") });
    await until(async () => existsSync(socket), Boolean, "the private session bus", READY_MS);
    const sinkLog = path.join(root, "sink.log");
    sink = spawnLogged("/usr/bin/python3", ["-I", SINK, address, notesLog], { env: { ...env, DBUS_SESSION_BUS_ADDRESS: address }, cwd: root, log: sinkLog });
    await until(async () => lines(sinkLog).includes("ready"), Boolean, "the notification sink", READY_MS);
  } catch (error) {
    await close();
    throw error;
  }
  return {
    env: { DBUS_SESSION_BUS_ADDRESS: address, PATH: `${bin}${path.delimiter}${env.PATH ?? ""}` },
    opened: () => lines(openedLog),
    notes: () => lines(notesLog).map((line) => JSON.parse(line) as Note),
    async click(id) {
      assert.ok(Number.isSafeInteger(id) && id > 0, "a shown notification has a positive id");
      await exec("/usr/bin/dbus-send", [`--bus=${address}`, "--type=method_call", "--dest=org.freedesktop.Notifications",
        "/org/freedesktop/Notifications", "org.aether.E2E.Invoke", `uint32:${id}`, "string:default"], { timeout: 10_000 });
    },
    close,
  };
}

/** Presses `keys` on the run's own compositor, for the native dialog that has the focus. */
export async function pressKeys(compositor: NodeJS.ProcessEnv, keys: readonly string[]): Promise<void> {
  assert.ok(keys.length > 0 && keys.every((key) => /^[A-Za-z]+$/.test(key)), "keys are key names");
  assert.ok(String(compositor.WAYLAND_DISPLAY ?? "").startsWith("/"), "the keyboard is the private compositor's");
  await exec("/usr/bin/wtype", keys.flatMap((key) => ["-k", key]), { env: compositor, timeout: 10_000 });
}

/** The titles of every window on the run's own compositor. */
export async function windowTitles(compositor: NodeJS.ProcessEnv): Promise<string[]> {
  const ipc = String(compositor.SWAYSOCK ?? "");
  assert.ok(ipc.startsWith("/"), "the private compositor's IPC socket is known");
  const { stdout } = await exec("/usr/bin/swaymsg", ["-s", ipc, "-t", "get_tree", "-r"], { timeout: 10_000 });
  const titles: string[] = [];
  const walk = (node: { name?: unknown; pid?: unknown; nodes?: unknown[]; floating_nodes?: unknown[] }) => {
    if (typeof node.pid === "number" && typeof node.name === "string") titles.push(node.name);
    for (const child of [...(node.nodes ?? []), ...(node.floating_nodes ?? [])]) walk(child as typeof node);
  };
  walk(JSON.parse(stdout) as Parameters<typeof walk>[0]);
  return titles;
}

/** Moves the focus to the next window on the run's own compositor, as a person switching windows would. */
export async function focusNextWindow(compositor: NodeJS.ProcessEnv): Promise<void> {
  const ipc = String(compositor.SWAYSOCK ?? "");
  assert.ok(ipc.startsWith("/"), "the private compositor's IPC socket is known");
  assert.ok((await windowTitles(compositor)).length >= 2, "another window is there to take the focus");
  await exec("/usr/bin/swaymsg", ["-s", ipc, "focus", "next"], { timeout: 10_000 });
}
