import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import type { DesktopBridge } from "@aether-ui/protocol";

function preload() {
  const handlers = new Map<string, (...args: unknown[]) => void>();
  let desktop: DesktopBridge | undefined;
  let next = 0;
  const closed = new Set<string>();
  const ipcRenderer = {
    on: (channel: string, handler: (...args: unknown[]) => void) => handlers.set(channel, handler),
    invoke: async (channel: string, conn: string) => {
      if (channel === "desktop:connect") return `c${++next}`;
      if (channel === "desktop:request" && closed.has(conn)) throw new Error("connection is closed");
      return undefined;
    },
  };
  const contextBridge = { exposeInMainWorld: (_name: string, bridge: DesktopBridge) => { desktop = bridge; } };
  const source = readFileSync(new URL("../../src/preload/index.cjs", import.meta.url), "utf8");
  runInNewContext(source, { require: () => ({ ipcRenderer, contextBridge }), queueMicrotask });
  assert.ok(desktop);
  return { desktop, emit: (conn: string) => { closed.add(conn); handlers.get("desktop:closed")?.(null, conn); } };
}

test("preload delivers a close only for a connection owned by the window", async () => {
  const { desktop, emit } = preload();
  const conn = await desktop.connect("local");
  let closed = 0;
  assert.throws(() => desktop.onClose("other-window", () => {}), /owned/);
  desktop.onClose(conn, () => { closed += 1; });
  emit("other-window");
  assert.equal(closed, 0);
  emit(conn);
  emit(conn);
  assert.equal(closed, 1);
});

test("a close before registration remains visible to the next request", async () => {
  const { desktop, emit } = preload();
  const conn = await desktop.connect("local");
  emit(conn);
  let notified = false;
  desktop.onClose(conn, () => { notified = true; });
  await assert.rejects(desktop.request(conn, { cmd: "hello", req: 1 }), /connection is closed/);
  assert.equal(notified, false);
});

test("preload suppresses notifications after explicit close", async () => {
  const { desktop, emit } = preload();
  const conn = await desktop.connect("local");
  let closed = 0;
  desktop.onClose(conn, () => { closed += 1; });
  await desktop.close(conn);
  emit(conn);
  assert.equal(closed, 0);
  assert.throws(() => desktop.onClose(conn, () => {}), /owned/);
});
