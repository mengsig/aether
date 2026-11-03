import type { ConnectionInfo } from "@aether-ui/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { computeCapabilities, sameCapabilities } from "./capabilities.ts";
import { ConnectionManager } from "./manager.ts";
import { INITIAL_STATE, Store } from "./store.ts";
import { FakeDaemon } from "./fixtures/fake-daemon.ts";
import F from "./fixtures/aether-frames.json";

const storage = { get: async () => undefined, set: async () => {} };

function observerDaemon() {
  const store = new Store(INITIAL_STATE, (flush) => flush());
  const daemon = new FakeDaemon((command) => {
    switch (command.cmd) {
      case "hello": return { ok: F.welcome_observer };
      case "subscribe": return { ok: F.subscribed_durable };
      case "get_attention": return { ok: F.attention };
      case "get_tree": return { ok: F.tree };
      case "get_agent": return { ok: { reply: "agent", data: { id: command.id } } };
      default: throw new Error(`unscripted ${String(command.cmd)}`);
    }
  });
  const manager = new ConnectionManager({ store, host: { kind: "browser", storage }, clientVersion: "test", random: () => 0 });
  manager.add({ daemon: "aether", transport: daemon.transport, target: "wss://host/ws", label: "host" });
  return { store, daemon, manager };
}

beforeEach(() => { vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] }); });
afterEach(() => { vi.useRealTimers(); });

describe("observer standing", () => {
  it("disables can.operate and refuses changes before they are sent", async () => {
    const { store, daemon, manager } = observerDaemon();
    manager.start();
    await vi.advanceTimersByTimeAsync(0);
    const caps = store.get().capabilities;
    expect(caps["standing.aether:inst-a"]).toBe("observer");
    expect(caps["can.operate.aether:inst-a"]).toBe(false);
    expect(caps["can.host_admin.aether:inst-a"]).toBe(false);
    expect(caps["aether.connected"]).toBe(true);
    const sent = daemon.sent.length;
    await expect(manager.send("aether:inst-a", { cmd: "send_message", to: F.agent, body: "hi" })).rejects.toMatchObject({ code: "forbidden" });
    expect(daemon.sent).toHaveLength(sent);
    expect(store.get().aether["aether:inst-a"]?.outbox).toEqual([]);
    await expect(manager.send("aether:inst-a", { cmd: "get_agent", id: F.agent })).resolves.toMatchObject({ reply: "agent" });
  });
});

describe("computeCapabilities", () => {
  const ready = (info: Partial<ConnectionInfo> & Pick<ConnectionInfo, "id" | "daemon">): ConnectionInfo => ({ state: "ready", features: [], label: "x", ...info });

  it("derives every flag from the host, connections and settings", () => {
    const caps = computeCapabilities({
      host: "electron", desktop: true, managedStart: true,
      connections: {
        "aether:i": ready({ id: "aether:i", daemon: "aether", standing: "local", features: ["deltas", "noesis"] }),
        "archon:h": ready({ id: "archon:h", daemon: "archon", standing: "operator", features: ["changes"] }),
        "aether:j": { id: "aether:j", daemon: "aether", state: "backoff", features: ["x"], label: "j", standing: "operator" },
      },
      noesis: { "aether:i": { enabled: true, effective: { "noesis.engineering_context": true } } },
    });
    expect(caps).toMatchObject({
      "host.electron": true, "host.browser": false, "aether.connected": true, "archon.connected": true,
      "aether.feature.deltas": true, "archon.feature.changes": true,
      "can.operate.aether:i": true, "can.host_admin.aether:i": true, "can.operate.archon:h": true, "can.host_admin.archon:h": false,
      "noesis.available": true, "noesis.engineering_context": true, "desktop.managed_start": true,
    });
    expect(caps["aether.feature.x"]).toBeUndefined();
    expect(caps["standing.aether:j"]).toBeUndefined();
    expect(Object.isFrozen(caps)).toBe(true);
  });

  it("keeps noesis off unless a connection with the feature reports it enabled", () => {
    const base = { host: "browser" as const, desktop: false, managedStart: true };
    const connections = { "aether:i": ready({ id: "aether:i", daemon: "aether", standing: "operator" }) };
    const caps = computeCapabilities({ ...base, connections, noesis: { "aether:i": { enabled: true } } });
    expect(caps["noesis.available"]).toBe(false);
    expect(caps["desktop.managed_start"]).toBe(false);
    expect(() => computeCapabilities({ ...base, desktop: true, connections: {}, noesis: {} })).toThrow(/desktop bridge/);
  });

  it("compares by value", () => {
    const input = { host: "browser" as const, desktop: false, managedStart: false, connections: {}, noesis: {} };
    expect(sameCapabilities(computeCapabilities(input), computeCapabilities(input))).toBe(true);
    expect(sameCapabilities(computeCapabilities(input), computeCapabilities({ ...input, host: "electron" }))).toBe(false);
  });
});
