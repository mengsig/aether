import { DaemonError, type App } from "@aether-ui/core";
import type { DesktopSetup, SetupResult, SetupStep } from "@aether-ui/protocol";
import { describe, expect, it, vi } from "vitest";
import { archonHealth, archonTools, providerReadiness } from "./readiness.ts";
import { desktopSetup, runStep, SetupError, type SetupFacts } from "./steps.ts";

type Json = Record<string, unknown>;

const claude = { provider: "claude-code", name: "Claude Code", configured: true, enabled: true, engine: true, models: 3, protocol: "engine", open_model_list: false, health: { provider: "claude-code", running: 0 } };
const signedIn = { provider: "claude-code", provider_name: "Claude Code", configured: true };
const archonServer = { name: "archon", state: "ready", transport: "stdio", tools: [{ name: "a" }, { name: "b" }] };

interface Daemons {
  readonly mcp?: unknown[];
  readonly status?: Json[];
  readonly providers?: unknown[];
  readonly auths?: unknown[];
}

function fakeApp(daemons: Daemons, ready: readonly ("aether" | "archon")[] = ["aether", "archon"]) {
  const sent: Array<[string, Json]> = [];
  const statuses = [...(daemons.status ?? [{ mcp: { failed_refreshes: 0, health: { state: "ready", tools: 2 } } }])];
  const connections = (["aether", "archon"] as const).map((daemon) => ({
    id: `${daemon}:local`, endpoint: { daemon, transport: { id: "ipc" } }, current: { state: ready.includes(daemon) ? "ready" : "connecting" },
  }));
  const answer = (command: Json): Json => {
    switch (command.cmd) {
      case "mcp_servers": return { reply: "mcp_servers", data: daemons.mcp ?? [archonServer] };
      case "status": return { reply: "status", data: statuses.length > 1 ? statuses.shift() : statuses[0] };
      case "providers": return { reply: "providers", data: daemons.providers ?? [claude] };
      case "auth_status": return { reply: "auth", data: daemons.auths ?? [signedIn] };
      default: throw new Error(`unscripted ${String(command.cmd)}`);
    }
  };
  const app = {
    host: { kind: "electron", desktop: {} },
    rediscover: vi.fn(async () => 0),
    manager: {
      all: () => connections,
      send: vi.fn(async (conn: string, command: Json) => { sent.push([conn, command]); return answer(command); }),
    },
  } as unknown as App;
  return { app, sent };
}

function bridge(results: Partial<Record<SetupStep, SetupResult | Error>>): DesktopSetup {
  return {
    setup: vi.fn(async (step: SetupStep) => {
      const result = results[step];
      if (result instanceof Error) throw result;
      if (result === undefined) throw new Error(`unscripted ${step}`);
      return result;
    }),
  };
}

const quick = { waitMs: 50, sleep: async () => {} };

describe("setup steps", () => {
  it("names a missing tool and how to get it, and never claims success", async () => {
    const setup = bridge({ tools: { step: "tools", tools: [{ name: "aether", found: true, version: "1.3.0" }, { name: "archon", found: false }] } });
    const error = await runStep("tools", {}, { app: fakeApp({}).app, setup }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(SetupError);
    expect((error as SetupError).message).toBe("Archon is not installed on this computer yet. Install it, then check again.");
    expect((error as SetupError).missing).toEqual(["archon"]);
  });

  it("passes the bridge's plain refusal through, and reconnects after starting", async () => {
    const { app } = fakeApp({});
    const setup = bridge({ connect: new Error("Cancelled"), start: { step: "start", restarted: true, daemons: [] } });
    await expect(runStep("connect", {}, { app, setup })).rejects.toThrow(/^Cancelled$/);
    await expect(runStep("start", {}, { app, setup })).resolves.toEqual({ restarted: true });
    expect(app.rediscover).toHaveBeenCalledTimes(1);
  });

  it("checks Aether's tool count, Archon's own view and a ready provider", async () => {
    const { app, sent } = fakeApp({ status: [{ mcp: { failed_refreshes: 0, health: { state: "unknown" } } }, { mcp: { failed_refreshes: 0, health: { state: "ready", tools: 2 } } }] });
    const facts = await runStep("check", { mode: "shared" }, { app, setup: bridge({}), ...quick });
    expect(facts).toEqual({ archonTools: 2, providers: [{ id: "claude-code", name: "Claude Code", readiness: { ready: true, tone: "success", text: "Ready" } }] });
    expect(sent[0]).toEqual(["aether:local", { cmd: "mcp_servers", refresh: true }]);
    expect(sent.filter(([, c]) => c.cmd === "status")).toHaveLength(2);
  });

  it("sends the person back to connecting when Aether lacks Archon's tools", async () => {
    const { app } = fakeApp({ mcp: [] });
    const error = (await runStep("check", { mode: "shared" }, { app, setup: bridge({}), ...quick }).catch((e: unknown) => e)) as SetupError;
    expect(error.message).toMatch(/^Archon's tools are not connected to Aether yet\./);
    expect(error.retryFrom).toBe("connect");
  });

  it("offers sign-in when no provider can run work, and gives up waiting on a daemon that never answers", async () => {
    const { app } = fakeApp({ auths: [{ ...signedIn, configured: false }] });
    const error = (await runStep("check", {} satisfies SetupFacts, { app, setup: bridge({}), ...quick }).catch((e: unknown) => e)) as SetupError;
    expect(error.message).toBe("No model provider is ready to run work yet. Sign in to one, then check again.");
    expect(error.signIn).toEqual(["claude"]);
    const silent = fakeApp({}, ["aether"]).app;
    await expect(runStep("check", {}, { app: silent, setup: bridge({}), ...quick })).rejects.toThrow("Archon started, but the app could not connect to it. Try again in a moment.");
  });

  it("asks again when Aether's link closes mid-check after a restart, and says so plainly if it never comes back", async () => {
    const { app, sent } = fakeApp({});
    const send = vi.mocked(app.manager.send);
    const closed = () => new DaemonError("disconnected", "The link closed before the answer: local:1 is not an open local connection");
    send.mockRejectedValueOnce(closed()).mockRejectedValueOnce(closed());
    const facts = await runStep("check", { mode: "shared" }, { app, setup: bridge({}), waitMs: 5_000, sleep: async () => {} });
    expect(facts.archonTools).toBe(2);
    expect(sent.map(([, c]) => c.cmd)).toEqual(["mcp_servers", "status", "providers", "auth_status"]);
    send.mockImplementation(async () => { throw closed(); });
    const error = (await runStep("check", { mode: "shared" }, { app, setup: bridge({}), ...quick }).catch((e: unknown) => e)) as SetupError;
    expect(error).toBeInstanceOf(SetupError);
    expect(error.message).toBe("Aether stopped answering while the app checked it. Try again in a moment.");
    send.mockReset().mockRejectedValue(new DaemonError("forbidden", "not allowed"));
    await expect(runStep("check", { mode: "shared" }, { app, setup: bridge({}), waitMs: 5_000, sleep: async () => {} })).rejects.toThrow(/^not allowed$/);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("finds the bridge only on a desktop that has it", () => {
    expect(desktopSetup({ host: { kind: "browser" } } as unknown as App)).toBeNull();
    expect(desktopSetup({ host: { kind: "electron", desktop: {} } } as unknown as App)).toBeNull();
    const setup = vi.fn();
    expect(desktopSetup({ host: { kind: "electron", desktop: { setup } } } as unknown as App)).not.toBeNull();
  });
});

describe("readiness", () => {
  it("UX-003: never calls a provider ready unless it can run work now", () => {
    expect(providerReadiness({ ...claude, enabled: false }, undefined).text).toBe("Turned off");
    expect(providerReadiness({ ...claude, configured: false }, undefined)).toMatchObject({ ready: false, text: "Not signed in", signIn: "claude" });
    expect(providerReadiness(claude, { ...signedIn, configured: false })).toMatchObject({ ready: false, text: "Not signed in" });
    expect(providerReadiness({ ...claude, provider: "openai", engine: false, configured: false }, undefined)).toMatchObject({ ready: false, text: "Not set up" });
    expect(providerReadiness({ ...claude, health: { ...claude.health, backoff: { reason: "rate limited", until: "x" } } } as never, signedIn)).toMatchObject({ ready: false, text: "Paused for now" });
    expect(providerReadiness({ ...claude, health: { ...claude.health, running: 2 } }, signedIn)).toMatchObject({ ready: true, text: "Ready, 2 running" });
  });

  it("reads Archon's tools from both daemons' answers", () => {
    expect(archonTools([])).toMatchObject({ state: "missing" });
    expect(archonTools([{ ...archonServer, state: "failed", error: "spawn failed" }] as never)).toMatchObject({ state: "failed", detail: "Aether could not reach Archon's tools: spawn failed" });
    expect(archonTools([{ ...archonServer, tools: [] }] as never)).toMatchObject({ state: "failed" });
    expect(archonTools([archonServer] as never)).toEqual({ state: "ready", tools: 2 });
    expect(archonHealth(null)).toMatchObject({ state: "starting" });
    expect(archonHealth({ failed_refreshes: 0, health: { state: "not_loaded" } })).toMatchObject({ state: "missing" });
    expect(archonHealth({ failed_refreshes: 0, health: { state: "ready", tools: 41 } })).toEqual({ state: "ready", tools: 41 });
  });
});
