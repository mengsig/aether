import type { DesktopBridge, PrivilegedAction } from "@aether-ui/protocol";
import { describe, expect, it, vi } from "vitest";
import { createApp } from "./app.ts";
import { FakeDaemon } from "./fixtures/fake-daemon.ts";
import type { Json } from "./guards.ts";

const storage = { get: async () => undefined, set: async () => {} };
const welcome = (role: string | null): Json => ({ reply: "welcome", data: {
  protocol_version: 26, min_protocol_version: 17, daemon_version: "t", pid: 1, schema_version: 1, instance_id: "i1",
  features: [], idle_timeout_secs: null, ...(role ? { device: { id: "dev_x", name: "x", role } } : {}),
} });

function daemon(role: string | null, id: "ipc" | "ws-aether"): FakeDaemon {
  return new FakeDaemon((command) => {
    if (command.cmd === "hello") return { ok: welcome(role) };
    if (command.cmd === "subscribe") return { ok: { reply: "subscribed", data: { id: 1, last_seq: 0 } } };
    if (command.cmd === "get_attention") return { ok: { reply: "attention", data: { agents: [], approvals: [], questions: [], last_seq: 0 } } };
    if (command.cmd === "list_agents_page") return { ok: { reply: "agents_page", data: { agents: [] } } };
    return undefined;
  }, id);
}

const settle = async () => { for (let i = 0; i < 30; i += 1) await Promise.resolve(); };

describe("app.privileged", () => {
  it("sends a host-only action through the desktop bridge on a fresh local connection, then closes it", async () => {
    const calls: PrivilegedAction[] = [];
    const desktop = {
      version: 1, connect: vi.fn(async () => "bridge-7"), close: vi.fn(async () => {}),
      privileged: vi.fn(async (action: PrivilegedAction) => { calls.push(action); return { reply: "ok" }; }),
      getPreference: async () => false,
    } as unknown as DesktopBridge;
    const fake = daemon(null, "ipc");
    const app = createApp({ kind: "electron", desktop, storage }, { scheduler: (f) => f(), endpoints: [{ daemon: "aether", transport: fake.transport, target: "local-1", label: "studio" }] });
    await app.start();
    await settle();
    expect(app.store.get().connections["aether:i1"]?.standing).toBe("local");
    await expect(app.privileged("aether:i1", { kind: "memory.accept", memory: "mem_1" })).resolves.toEqual({ reply: "ok" });
    expect(desktop.connect).toHaveBeenCalledWith("local-1");
    expect(calls).toEqual([{ kind: "memory.accept", memory: "mem_1", conn: "bridge-7" }]);
    expect(desktop.close).toHaveBeenCalledWith("bridge-7");
  });

  it("refuses and reports in a browser, where no connection is the host's own", async () => {
    const fake = daemon("operator", "ws-aether");
    const app = createApp({ kind: "browser", storage }, { scheduler: (f) => f(), endpoints: [{ daemon: "aether", transport: fake.transport, target: "wss://h/ws", label: "h" }] });
    await app.start();
    await settle();
    await expect(app.privileged("aether:i1", { kind: "device.revoke", device: "dev_1" })).rejects.toMatchObject({ code: "forbidden" });
    expect(app.store.get().reports.at(-1)?.message).toMatch(/desktop app/);
  });
});
