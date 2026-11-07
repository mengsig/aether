// The setup steps as the app drives them, over fake CLIs, probes and confirmations: connect
// writes Archon's config and owes a reload only when needed; start restarts Aether only if agreed.

import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { Bridge, type BridgeHost } from "../../src/bridge/bridge.ts";
import type { FoundDaemon } from "../../src/discovery/daemons.ts";
import { ARCHON_CONFIG_TEXT } from "../../src/lifecycle/archon-connect.ts";
import type { CommandResult } from "../../src/lifecycle/commands.ts";
import { Setup } from "../../src/lifecycle/setup.ts";
import type { Confirmation } from "../../src/security/confirm.ts";
import { BridgeRefusal } from "../../src/security/validate.ts";
import { APP_SENDER, VALIDATOR } from "../security/support.ts";

const ran = (stdout: string, code = 0, stderr = ""): CommandResult => ({ found: true, code, stdout, stderr });
const VERSIONS = { "aether version": ran("aether 1.3.0 (protocol 24)\n"), "archon --version": ran("archon 1.4.12\n") };
const mcpHealth = (level: string) => ({ check: "mcp health", level, detail: "aetherd has not loaded the archon server" });
const sharedAnswer = (level: string, registration = "added") =>
  ran(JSON.stringify({ mode: "shared", config: "/c/config.toml", registration, command: ["archon", "mcp"], mcp_health: mcpHealth(level) }));
const dedicatedAnswer = (level: string) => ran(JSON.stringify({ mode: "dedicated", config: "/c/config.toml", mcp_health: mcpHealth(level) }));

const DAEMONS: FoundDaemon[] = [
  { id: "aether:local", kind: "aether", label: "aetherd", socket: "/run/test/aetherd.sock", reachable: true, version: "1.3.0" },
  { id: "archon:local", kind: "archon", label: "archond", socket: "/run/test/archond.sock", reachable: true },
];

interface Harness {
  readonly setup: Setup;
  /** What ran, in order: commands, confirmations, the preference and managed starts. */
  readonly events: string[];
  readonly shown: Confirmation[];
  readonly config: string;
  /** Whether Aether answers; `aether stop` turns it off after `stopsAfter` more probes. */
  readonly state: { up: boolean; stopsAfter: number };
  cleanup(): void;
}

interface HarnessOptions {
  answers?: Record<string, CommandResult>;
  up?: boolean;
  confirm?: boolean;
  /** Probes Aether still answers after `aether stop`. */
  stopProbes?: number;
  stopDeadlineMs?: number;
}

function harness(options: HarnessOptions): Harness {
  const dir = mkdtempSync(path.join(os.tmpdir(), "e2-setup-"));
  const events: string[] = [];
  const shown: Confirmation[] = [];
  const state = { up: options.up ?? false, stopsAfter: -1 };
  const answers: Record<string, CommandResult> = { ...VERSIONS, "aether stop": ran(""), ...options.answers };
  const config = path.join(dir, "config", "archon", "config.toml");
  const setup = new Setup({
    run: async (tool, args) => {
      const key = `${tool} ${args.join(" ")}`;
      events.push(`run ${key}`);
      if (key === "aether stop") state.stopsAfter = options.stopProbes ?? 2;
      const answer = answers[key];
      if (answer === undefined) throw new Error(`unexpected command: ${key}`);
      return answer;
    },
    archonConfig: config,
    isReachable: async (kind) => {
      if (kind !== "aether") throw new Error("setup only ever probes Aether");
      if (state.stopsAfter === 0) state.up = false;
      if (state.stopsAfter > 0) state.stopsAfter -= 1;
      return state.up;
    },
    enableManagedStart: async () => { events.push("enable managed start"); },
    confirm: async (_owner, confirmation) => {
      events.push("confirm");
      shown.push(confirmation);
      return options.confirm ?? false;
    },
    start: async (kind) => { events.push(`start ${kind}`); },
    findDaemons: async () => DAEMONS,
    stopDeadlineMs: options.stopDeadlineMs ?? 2_000,
    pollMs: 5,
  });
  return { setup, events, shown, config, state, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

test("the tools step reports both CLIs and changes nothing", async () => {
  const h = harness({ answers: { "archon --version": { found: false } } });
  try {
    assert.deepEqual(await h.setup.run(1, "tools"), {
      step: "tools", tools: [{ name: "aether", found: true, version: "1.3.0" }, { name: "archon", found: false }],
    });
    assert.deepEqual(h.events, ["run aether version", "run archon --version"]);
    assert.equal(existsSync(h.config), false);
  } finally {
    h.cleanup();
  }
});

test("connect refuses plainly when a tool is missing, and runs nothing else", async () => {
  const h = harness({ answers: { "aether version": { found: false }, "archon --version": { found: false } } });
  try {
    await assert.rejects(h.setup.run(1, "connect"), (error: unknown) =>
      error instanceof BridgeRefusal && error.message === "Install Aether and Archon on this computer first, then try again.");
    assert.deepEqual(h.events, ["run aether version", "run archon --version"]);
    assert.equal(existsSync(h.config), false, "no config was written");
  } finally {
    h.cleanup();
  }
});

test("a confirmed connect writes Archon's config, runs archon setup and reports its registration", async () => {
  const h = harness({ answers: { "archon setup --json": sharedAnswer("ok", "already_present") }, confirm: true });
  try {
    assert.deepEqual(await h.setup.run(1, "connect"), { step: "connect", mode: "shared", registration: "already_present", reload_owed: false });
    assert.equal(readFileSync(h.config, "utf8"), ARCHON_CONFIG_TEXT);
    assert.deepEqual(h.events, ["run aether version", "run archon --version", "confirm", "run archon setup --json"]);
    assert.equal(h.shown[0]?.message, "Connect Archon's tools to Aether?");
  } finally {
    h.cleanup();
  }
});

test("a cancelled connect writes no config and runs no archon setup; a retry asks again", async () => {
  for (const up of [false, true]) {
    const h = harness({ answers: { "archon setup --json": sharedAnswer("fail") }, up, confirm: false });
    try {
      await assert.rejects(h.setup.run(1, "connect"), (error: unknown) => error instanceof BridgeRefusal && error.message === "Cancelled");
      assert.equal(existsSync(h.config), false, "Archon's config is untouched");
      assert.ok(!h.events.includes("run archon setup --json"), "Aether's config is untouched");
      assert.equal(h.shown[0]?.message, up ? "Connect Archon's tools and restart Aether?" : "Connect Archon's tools to Aether?");
      h.events.length = 0;
      await h.setup.run(1, "start");
      assert.deepEqual(h.events, ["enable managed start", "start aether", "start archon"], "no restart without consent");
      await assert.rejects(h.setup.run(1, "connect"), /Cancelled/);
      assert.equal(h.shown.length, 2);
    } finally {
      h.cleanup();
    }
  }
});

test("a failed archon setup is a refusal with the first line Archon wrote", async () => {
  const h = harness({ answers: { "archon setup --json": ran("", 1, "\nerror: aether is not installed\nmore detail\n") }, confirm: true });
  try {
    await assert.rejects(h.setup.run(1, "connect"), (error: unknown) => error instanceof BridgeRefusal
      && error.message === "Setup could not connect Archon's tools to Aether. Archon said: error: aether is not installed");
    const garbled = harness({ answers: { "archon setup --json": ran("{\"mode\":") }, confirm: true });
    await assert.rejects(garbled.setup.run(1, "connect"), /does not understand/);
    garbled.cleanup();
  } finally {
    h.cleanup();
  }
});

test("a reload is owed only in shared mode, with Aether running and Archon's tools not loaded", async () => {
  for (const mode of ["shared", "dedicated"] as const) {
    for (const up of [true, false]) {
      for (const level of ["ok", "warn", "fail"]) {
        const answer = mode === "shared" ? sharedAnswer(level) : dedicatedAnswer(level);
        const h = harness({ answers: { "archon setup --json": answer }, up, confirm: true });
        try {
          const result = await h.setup.run(1, "connect");
          assert.equal(result.step === "connect" && result.reload_owed, mode === "shared" && up && level === "fail", `${mode} ${up} ${level}`);
          assert.equal(result.step === "connect" && result.registration, mode === "shared" ? "added" : "dedicated");
        } finally {
          h.cleanup();
        }
      }
    }
  }
});

test("start turns on managed start and starts both without a restart when none is owed", async () => {
  const h = harness({ answers: { "archon setup --json": sharedAnswer("ok") }, up: true, confirm: true });
  try {
    await h.setup.run(1, "connect");
    h.events.length = 0;
    const result = await h.setup.run(1, "start");
    assert.deepEqual(h.events, ["enable managed start", "start aether", "start archon"]);
    assert.deepEqual(result, { step: "start", restarted: false, daemons: [
      { id: "aether:local", kind: "aether", label: "aetherd", reachable: true, version: "1.3.0" },
      { id: "archon:local", kind: "archon", label: "archond", reachable: true },
    ] });
    assert.ok(!JSON.stringify(result).includes("/run/test"), "no socket path leaves main");
  } finally {
    h.cleanup();
  }
});

test("a connect confirmed with its restart runs aether stop, waits until Aether is down, then starts both", async () => {
  const h = harness({ answers: { "archon setup --json": sharedAnswer("fail") }, up: true, confirm: true });
  try {
    await h.setup.run(1, "connect");
    h.events.length = 0;
    const result = await h.setup.run(7, "start");
    assert.equal(result.step === "start" && result.restarted, true);
    assert.deepEqual(h.events, ["enable managed start", "run aether stop", "start aether", "start archon"]);
    assert.equal(h.state.up, false, "start ran only once Aether stopped answering");
    assert.equal(h.shown.length, 1, "one confirmation covers connecting and restarting");
    assert.equal(h.shown[0]?.message, "Connect Archon's tools and restart Aether?");
    assert.match(h.shown[0]?.detail ?? "", /pick up where they left off/);
    h.events.length = 0;
    assert.equal((await h.setup.run(7, "start")).step === "start", true);
    assert.deepEqual(h.events, ["enable managed start", "start aether", "start archon"], "the reload is paid once");
  } finally {
    h.cleanup();
  }
});

test("an Aether that will not stop, or a failed stop, is a plain refusal and starts nothing", async () => {
  const h = harness({ answers: { "archon setup --json": sharedAnswer("fail") }, up: true, confirm: true, stopProbes: 1_000_000, stopDeadlineMs: 50 });
  try {
    await h.setup.run(1, "connect");
    const started = Date.now();
    await assert.rejects(h.setup.run(1, "start"), /taking a while to stop/);
    assert.ok(Date.now() - started < 2_000);
    assert.ok(!h.events.some((event) => event.startsWith("start ")), "nothing started while Aether still answered");
    const failing = harness({ answers: { "archon setup --json": sharedAnswer("fail"), "aether stop": ran("", 1, "no permission") }, up: true, confirm: true });
    await failing.setup.run(1, "connect");
    await assert.rejects(failing.setup.run(1, "start"), /Aether could not be restarted. Aether said: no permission/);
    assert.ok(!failing.events.some((event) => event.startsWith("start ")), "nothing started after a failed stop");
    failing.cleanup();
  } finally {
    h.cleanup();
  }
});

test("one setup runs at a time, and the bridge serves setup through the validator", async () => {
  let release = () => {};
  const blocked = new Promise<void>((resolve) => { release = resolve; });
  const h = harness({});
  const slow = new Setup({
    run: async () => { await blocked; return { found: false }; },
    archonConfig: h.config, isReachable: async () => false, enableManagedStart: async () => {},
    confirm: async () => false, start: async () => {}, findDaemons: async () => DAEMONS,
  });
  try {
    const first = slow.run(1, "tools");
    await assert.rejects(slow.run(1, "connect"), /Setup is already running/);
    release();
    assert.equal((await first).step, "tools");
    const host = { setup: (owner, step) => h.setup.run(owner, step) } as Partial<BridgeHost> as BridgeHost;
    const bridge = new Bridge(VALIDATOR, host);
    assert.equal(((await bridge.handle(APP_SENDER, 1, "setup", ["tools"])) as { step: string }).step, "tools");
    await assert.rejects(bridge.handle(APP_SENDER, 1, "setup", ["restart"]), BridgeRefusal);
  } finally {
    h.cleanup();
  }
});
