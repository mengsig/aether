// A setup run is the app's, not a view's: it reports how each run ended exactly once, keeps the
// connection it started from across retries, and rides out a daemon that restarts mid-check.

import type { App } from "@aether-ui/core";
import type { DesktopSetup } from "@aether-ui/protocol";
import { expect, test, vi } from "vitest";
import { SetupRunner, type SetupOutcome } from "./useSetup.ts";

type Json = Record<string, unknown>;

const found = { step: "tools", tools: [{ name: "aether", found: true }, { name: "archon", found: true }] };
const claude = { provider: "claude-code", name: "Claude Code", configured: true, enabled: true, engine: true, models: 3, protocol: "engine", open_model_list: false, health: { provider: "claude-code", running: 0 } };

function toolsRunner(answer: () => Promise<unknown>) {
  const outcomes: SetupOutcome[] = [];
  const runner = new SetupRunner({} as App, { setup: answer } as unknown as DesktopSetup, ["tools"], (outcome) => outcomes.push(outcome));
  return { runner, outcomes };
}

/** Local Aether and Archon as the check reads them; `aetherState` and `providers` can change mid-run. */
function checkApp(daemons: { aetherState: string; providers: () => Promise<Json> }) {
  const conns = (["aether", "archon"] as const).map((daemon) => ({ id: `${daemon}:local`, endpoint: { daemon, transport: { id: "ipc" } }, current: { state: "ready" } }));
  const answer = async (command: Json): Promise<Json> => {
    switch (command.cmd) {
      case "mcp_servers": return { reply: "mcp_servers", data: [{ name: "archon", state: "ready", transport: "stdio", tools: [{ name: "a" }] }] };
      case "status": return { reply: "status", data: { mcp: { failed_refreshes: 0, health: { state: "ready", tools: 1 } } } };
      case "providers": return daemons.providers();
      case "auth_status": return { reply: "auth", data: [{ provider: "claude-code", provider_name: "Claude Code", configured: true }] };
      default: throw new Error(`unscripted ${String(command.cmd)}`);
    }
  };
  const all = () => conns.map((c) => (c.endpoint.daemon === "aether" ? { ...c, current: { state: daemons.aetherState } } : c));
  return { manager: { all, send: vi.fn(async (_conn: string, command: Json) => answer(command)) } } as unknown as App;
}

test("each run reports how it ended once, and starting again while it runs changes nothing", async () => {
  let answer: (value: unknown) => void = () => undefined;
  const { runner, outcomes } = toolsRunner(() => new Promise((resolve) => { answer = resolve; }));
  const first = runner.run(undefined, "aether:local");
  const second = runner.run(undefined, "aether:other");
  expect(runner.get()).toMatchObject({ busy: true, origin: "aether:local" });
  answer(found);
  await Promise.all([first, second]);
  expect(outcomes).toEqual([{ ok: true }]);
  expect(runner.get()).toMatchObject({ busy: false, finished: true });
});

test("a stopped run reports its step and reason once; a retry keeps its origin and tells finish listeners", async () => {
  let fail = true;
  const { runner, outcomes } = toolsRunner(async () => { if (fail) throw new Error("archon exited with status 2"); return found; });
  let finished = 0;
  const stop = runner.whenFinished(() => { finished += 1; });
  await runner.run(undefined, "aether:local");
  expect(outcomes).toHaveLength(1);
  expect(outcomes[0]).toMatchObject({ ok: false, step: "tools", error: { message: "archon exited with status 2" } });
  expect(finished).toBe(0);
  fail = false;
  await runner.run("tools");
  expect(runner.get().origin).toBe("aether:local");
  expect(outcomes.map((o) => o.ok)).toEqual([false, true]);
  expect(finished).toBe(1);
  stop();
  await runner.run("tools");
  expect(finished).toBe(1);
});

test("the check waits out an Aether restart, and a link lost mid-check stops it once with a retry that works", async () => {
  let lost = true;
  const daemons = {
    aetherState: "connecting",
    providers: async (): Promise<Json> => {
      if (lost) throw new Error("The connection to Aether was lost");
      return { reply: "providers", data: [claude] };
    },
  };
  const outcomes: SetupOutcome[] = [];
  const runner = new SetupRunner(checkApp(daemons), {} as DesktopSetup, ["check"], (outcome) => outcomes.push(outcome));
  const running = runner.run();
  setTimeout(() => { daemons.aetherState = "ready"; }, 50);
  await running;
  expect(outcomes).toHaveLength(1);
  expect(outcomes[0]).toMatchObject({ ok: false, step: "check", error: { message: "The connection to Aether was lost" } });
  lost = false;
  await runner.run("check");
  expect(outcomes.map((o) => o.ok)).toEqual([false, true]);
  expect(runner.get().facts.providers?.map((p) => p.id)).toEqual(["claude-code"]);
});
