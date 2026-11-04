// Managed start is off until desktop.managed_start is on, then runs the CLI's own start command
// detached, in its own session, with output in a log file. Launchers here are stub scripts.

import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
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

test("managed start launches in home and where discovery looks, whatever HOME and relative overrides say", async () => {
  const fx = fixture();
  try {
    const mainCwd = path.join(fx.dir, "main");
    const passwdHome = path.join(fx.dir, "pw");
    mkdirSync(mainCwd);
    mkdirSync(passwdHome);
    mkdirSync(path.join(mainCwd, "relhome"));
    launcher(fx, `echo "cwd=$(pwd -P) socket=$AETHER_SOCKET data=$AETHER_DATA_DIR home=$HOME" >> '${fx.marker}'; exit 0`);
    const deps = { isReachable: async () => false, logDir: fx.logDir };
    const off = managedStartOn(machineOf({ HOME: "" }, "linux", passwdHome), mainCwd, { ...deps, isEnabled: () => false });
    await assert.rejects(off.start("aether"), /is off/, "an empty HOME neither breaks startup nor enables anything");

    const cases: ReadonlyArray<readonly [Env, string, string]> = [
      [{ HOME: "", AETHER_SOCKET: "rel/a.sock" }, passwdHome, `socket=${mainCwd}/rel/a.sock data= home=`],
      [{ HOME: "relhome", AETHER_DATA_DIR: "" }, path.join(mainCwd, "relhome"), `socket= data=${mainCwd} home=${mainCwd}/relhome`],
      [{ HOME: passwdHome, AETHER_DATA_DIR: "d/../data" }, passwdHome, `socket= data=${mainCwd}/data home=${passwdHome}`],
    ];
    for (const [env, cwd, recorded] of cases) {
      rmSync(fx.marker, { force: true });
      const machine = machineOf({ PATH: `${fx.bin}:/usr/bin:/bin`, ...env }, "linux", passwdHome);
      await managedStartOn(machine, mainCwd, { ...deps, isEnabled: () => true }).start("aether");
      const line = readFileSync(fx.marker, "utf8").trim().split("\n").at(-1) ?? "";
      assert.equal(line, `cwd=${realpathSync(cwd)} ${recorded}`, JSON.stringify(env));
      const discovered = path.resolve(mainCwd, socketPath("aether", machine));
      const launched = path.resolve(cwd, socketPath("aether", machineOf(launchEnv(machine.env, mainCwd), "linux", passwdHome)));
      assert.equal(launched, discovered, `the daemon listens where discovery looks: ${JSON.stringify(env)}`);
    }
  } finally {
    fx.cleanup();
  }
});
