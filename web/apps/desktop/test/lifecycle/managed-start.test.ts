// Managed start is off until desktop.managed_start is on, then runs the CLI's own start command
// detached, in its own session, with output in a log file. Launchers here are stub scripts.

import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { Bridge, type BridgeHost } from "../../src/bridge/bridge.ts";
import { machineOf, socketPath, type Env } from "../../src/discovery/socket-paths.ts";
import { launchEnv, ManagedStart, managedStartOn } from "../../src/lifecycle/managed-start.ts";
import { PreferenceStore } from "../../src/main/preferences.ts";
import { APP_SENDER, VALIDATOR } from "../security/support.ts";

interface Fixture {
  readonly dir: string;
  readonly bin: string;
  readonly marker: string;
  readonly logDir: string;
  cleanup(): void;
}

function fixture(): Fixture {
  const dir = mkdtempSync(path.join(os.tmpdir(), "e2-start-"));
  const bin = path.join(dir, "bin");
  mkdirSync(bin);
  return { dir, bin, marker: path.join(dir, "ran"), logDir: path.join(dir, "logs"), cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

/** An `aether` on the fixture's PATH that records its arguments and session, then runs `body`. */
function launcher(fx: Fixture, body: string): void {
  const script = path.join(fx.bin, "aether");
  writeFileSync(script, `#!/bin/sh\necho "$$ $(ps -o sid= -p $$) $*" >> '${fx.marker}'\n${body}\n`);
  chmodSync(script, 0o755);
}

function managed(fx: Fixture, options: { enabled: () => boolean; reachable?: () => boolean }): ManagedStart {
  return new ManagedStart({
    isEnabled: options.enabled,
    isReachable: async () => options.reachable?.() ?? false,
    env: { PATH: `${fx.bin}:/usr/bin:/bin` },
    cwd: fx.dir,
    logDir: fx.logDir,
    deadlineMs: 5_000,
    pollMs: 20,
  });
}

test("managed start is off by default and needs the setting", async () => {
  const fx = fixture();
  try {
    launcher(fx, "exit 0");
    const preferences = new PreferenceStore(path.join(fx.dir, "preferences.json"), async () => {});
    assert.equal(preferences.get("desktop.managed_start"), null, "a fresh install has no setting");
    const start = managed(fx, { enabled: () => preferences.get("desktop.managed_start") === true });
    const host = { startDaemon: (kind) => start.start(kind) } as Partial<BridgeHost> as BridgeHost;
    const bridge = new Bridge(VALIDATOR, host);
    await assert.rejects(bridge.handle(APP_SENDER, 1, "startDaemon", ["aether"]), /Starting daemons from the app is off/);
    await preferences.set("desktop.managed_start", false);
    await assert.rejects(bridge.handle(APP_SENDER, 1, "startDaemon", ["aether"]), /is off/);
    assert.equal(existsSync(fx.marker), false, "nothing ran while the setting was off");
    await preferences.set("desktop.managed_start", true);
    await bridge.handle(APP_SENDER, 1, "startDaemon", ["aether"]);
    assert.equal(readFileSync(fx.marker, "utf8").trim().split(" ").at(-1), "daemon", "the CLI's own start command ran");
  } finally {
    fx.cleanup();
  }
});

test("the launcher runs detached in its own session, with its output in the log", async () => {
  const fx = fixture();
  try {
    launcher(fx, "echo started on stdout; echo warned on stderr >&2; exit 0");
    const start = managed(fx, { enabled: () => true });
    await start.start("aether");
    const [pid, sid, ...args] = readFileSync(fx.marker, "utf8").trim().split(/\s+/);
    assert.equal(sid, pid, "the launcher leads its own session");
    assert.deepEqual(args, ["daemon"]);
    const log = readFileSync(start.logFile("aether"), "utf8");
    assert.match(log, /the desktop app runs: aether daemon\nstarted on stdout\nwarned on stderr\n/);
  } finally {
    fx.cleanup();
  }
});

test("a failed or missing launcher is reported calmly, and a running daemon is left alone", async () => {
  const fx = fixture();
  try {
    await assert.rejects(managed(fx, { enabled: () => true }).start("aether"), /aether isn't installed, or isn't on the app's PATH/);
    launcher(fx, "echo config is broken >&2; exit 3");
    const start = managed(fx, { enabled: () => true });
    await assert.rejects(start.start("aether"), (error: Error) =>
      error.message.includes("aetherd didn't start (exit 3)") && error.message.includes(start.logFile("aether")));
    assert.match(readFileSync(start.logFile("aether"), "utf8"), /config is broken/);
    rmSync(fx.marker);
    await managed(fx, { enabled: () => true, reachable: () => true }).start("aether");
    assert.equal(existsSync(fx.marker), false, "a daemon that answers is not started again");
  } finally {
    fx.cleanup();
  }
});

test("concurrent starts of one daemon share one launch, and a slow one is never stopped", async () => {
  const fx = fixture();
  try {
    launcher(fx, `sleep 0.3; exit 0`);
    const start = managed(fx, { enabled: () => true });
    await Promise.all([start.start("aether"), start.start("aether")]);
    assert.equal(readFileSync(fx.marker, "utf8").trim().split("\n").length, 1, "one launcher ran");
    rmSync(fx.marker);
    launcher(fx, `sleep 1; echo finished >> '${fx.marker}'`);
    const slow = new ManagedStart({
      isEnabled: () => true, isReachable: async () => false, env: { PATH: `${fx.bin}:/usr/bin:/bin` },
      cwd: fx.dir, logDir: fx.logDir, deadlineMs: 100, pollMs: 20,
    });
    await assert.rejects(slow.start("aether"), /aetherd is still starting/);
    const end = Date.now() + 5_000;
    while (!readFileSync(fx.marker, "utf8").includes("finished") && Date.now() < end) await new Promise((done) => setTimeout(done, 50));
    assert.match(readFileSync(fx.marker, "utf8"), /finished/, "the launcher ran on after the app stopped waiting");
  } finally {
    fx.cleanup();
  }
});

test("an empty HOME neither breaks managed start nor enables it", async () => {
  const fx = fixture();
  try {
    const off = managedStartOn(machineOf({ HOME: "" }, "linux", fx.dir), fx.dir, {
      isEnabled: () => false, isReachable: async () => false, logDir: fx.logDir,
    });
    await assert.rejects(off.start("aether"), /is off/);
    assert.deepEqual(launchEnv({ HOME: "", AETHER_SOCKET: "", AETHER_DATA_DIR: "", ARCHON_SOCKET: "/abs/r.sock" }, "/main"),
      { HOME: "", AETHER_SOCKET: "", AETHER_DATA_DIR: "/main", ARCHON_SOCKET: "/abs/r.sock" });
    assert.deepEqual(launchEnv({ HOME: "h", ARCHON_DATA_DIR: "x/../d", AETHER_SOCKET: "l/../a.sock" }, "/"),
      { HOME: "/h", ARCHON_DATA_DIR: "/x/../d", AETHER_SOCKET: "/l/../a.sock" }, "prefixed, never normalized");
  } finally {
    fx.cleanup();
  }
});

test("relative config dirs keep meaning main's directory, not home", () => {
  assert.deepEqual(
    launchEnv({ HOME: "/home/u", AETHER_CONFIG_DIR: "cfg", ARCHON_CONFIG_DIR: "l/../c" }, "/project"),
    { HOME: "/home/u", AETHER_CONFIG_DIR: "/project/cfg", ARCHON_CONFIG_DIR: "/project/l/../c" },
  );
  assert.deepEqual(
    launchEnv({ AETHER_CONFIG_DIR: "/abs/a", ARCHON_CONFIG_DIR: "/abs/r" }, "/project"),
    { AETHER_CONFIG_DIR: "/abs/a", ARCHON_CONFIG_DIR: "/abs/r" }, "absolute ones are untouched",
  );
  assert.deepEqual(
    launchEnv({ AETHER_CONFIG_DIR: "", ARCHON_CONFIG_DIR: "" }, "/project"),
    { AETHER_CONFIG_DIR: "", ARCHON_CONFIG_DIR: "/project" }, "empty: as each CLI reads it",
  );
});

/** A launcher for `kind` that makes its socket and data dir and records where it ran. */
function placingLauncher(fx: Fixture, kind: "aether" | "archon"): void {
  const upper = kind.toUpperCase();
  const script = path.join(fx.bin, kind);
  writeFileSync(script, `#!/bin/sh
mkdir -p "$${upper}_DATA_DIR" && touch "$${upper}_DATA_DIR/state" "$${upper}_SOCKET"
cat "$${upper}_CONFIG_DIR/config" >> '${fx.marker}.config'
pwd -P > '${fx.marker}'
`);
  chmodSync(script, 0o755);
}

test("managed start launches where discovery looks, through symlinks and relative overrides", async () => {
  const fx = fixture();
  try {
    // main runs in main/; main/link points into target/child, so link/.. is target, not main.
    const main = path.join(fx.dir, "main");
    const target = path.join(main, "target");
    mkdirSync(path.join(target, "child"), { recursive: true });
    mkdirSync(path.join(target, "home"));
    symlinkSync(path.join(target, "child"), path.join(main, "link"));
    for (const kind of ["aether", "archon"] as const) {
      placingLauncher(fx, kind);
      const upper = kind.toUpperCase();
      const env: Env = {
        PATH: `${fx.bin}:/usr/bin:/bin`, HOME: "link/../home",
        [`${upper}_SOCKET`]: `link/../${kind}.sock`, [`${upper}_DATA_DIR`]: `link/../${kind}-data`,
        [`${upper}_CONFIG_DIR`]: `link/../${kind}-config`,
      };
      mkdirSync(path.join(target, `${kind}-config`));
      writeFileSync(path.join(target, `${kind}-config`, "config"), `${kind} as main reads it\n`);
      const machine = machineOf(env, "linux", fx.dir);
      await managedStartOn(machine, main, { isEnabled: () => true, isReachable: async () => false, logDir: fx.logDir }).start(kind);
      assert.equal(readFileSync(fx.marker, "utf8").trim(), realpathSync(path.join(target, "home")), `${kind} ran in HOME as main reads it`);
      const discovered = socketPath(kind, machine);
      assert.equal(discovered, `link/../${kind}.sock`, "discovery reads the relative path from main's directory");
      assert.ok(existsSync(path.join(target, `${kind}.sock`)), `${kind}'s socket is where main's ${discovered} leads`);
      assert.ok(existsSync(path.join(target, `${kind}-data`, "state")), `${kind}'s data is where main's override leads`);
      assert.match(readFileSync(`${fx.marker}.config`, "utf8"), new RegExp(`^${kind} as main reads it$`, "m"), `${kind} read main's config`);
      assert.ok(!existsSync(path.join(main, `${kind}.sock`)) && !existsSync(path.join(main, `${kind}-data`)), "nothing went to a normalized path");
    }
  } finally {
    fx.cleanup();
  }
});
