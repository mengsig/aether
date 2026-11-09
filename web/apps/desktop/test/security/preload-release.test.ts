import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import type { DesktopBridge } from "@aether-ui/protocol";

function required<T>(value: T | undefined): T {
  assert.ok(value);
  return value;
}

function preload() {
  const handlers = new Map<string, (...args: unknown[]) => void>();
  const pending: Array<{ resolve: () => void; reject: (error: Error) => void }> = [];
  let desktop: DesktopBridge | undefined;
  const ipcRenderer = {
    on: (channel: string, handler: (...args: unknown[]) => void) => handlers.set(channel, handler),
    invoke: (channel: string) => {
      if (channel === "desktop:connect") return Promise.resolve("c");
      if (channel === "desktop:subscribe") return new Promise<void>((resolve, reject) => pending.push({ resolve, reject }));
      return Promise.resolve();
    },
  };
  const contextBridge = { exposeInMainWorld: (_name: string, bridge: DesktopBridge) => { desktop = bridge; } };
  runInNewContext(readFileSync(new URL("../../src/preload/index.cjs", import.meta.url), "utf8"), { require: () => ({ ipcRenderer, contextBridge }), queueMicrotask });
  assert.ok(desktop);
  return { desktop, pending, emit: (channel: string, frame?: unknown) => handlers.get(`desktop:${channel}`)?.(null, "c", frame) };
}

test("shared callbacks stay live until their final release, including repeated agent switches", async () => {
  const s = preload();
  const conn = await s.desktop.connect("local");
  const seen: unknown[] = [];
  const callback = (frame: unknown) => seen.push(frame);
  const first = s.desktop.subscribe(conn, { cmd: "subscribe", req: 1 }, callback);
  const second = s.desktop.subscribe(conn, { cmd: "subscribe", req: 2 }, callback);
  s.emit("push", 1);
  required(s.pending[0]).resolve(); required(s.pending[1]).resolve();
  const releaseFirst = await first; const releaseSecond = await second;
  releaseFirst(); releaseFirst();
  s.emit("push", 2);
  const third = s.desktop.subscribe(conn, { cmd: "subscribe", req: 3 }, callback);
  required(s.pending[2]).resolve();
  const releaseThird = await third;
  releaseSecond();
  s.emit("push", 3);
  releaseThird();
  s.emit("push", 4);
  assert.deepEqual(seen, [1, 2, 3]);
});

test("failed subscribe and connection close remove holds even when subscribe is pending", async () => {
  const s = preload();
  const conn = await s.desktop.connect("local");
  const seen: unknown[] = [];
  const callback = (frame: unknown) => seen.push(frame);
  const failed = s.desktop.subscribe(conn, { cmd: "subscribe", req: 1 }, callback);
  required(s.pending[0]).reject(new Error("failed"));
  await assert.rejects(failed, /failed/);
  s.emit("push", 1);
  const pending = s.desktop.subscribe(conn, { cmd: "subscribe", req: 2 }, callback);
  s.emit("closed");
  s.emit("push", 2);
  required(s.pending[1]).resolve();
  (await pending)();
  assert.deepEqual(seen, []);
});

test("explicit close stops pushes while a subscribe is pending", async () => {
  const s = preload();
  const conn = await s.desktop.connect("local");
  let calls = 0;
  const pending = s.desktop.subscribe(conn, { cmd: "subscribe", req: 1 }, () => { calls++; });
  await s.desktop.close(conn);
  s.emit("push", 1);
  required(s.pending[0]).resolve();
  (await pending)();
  assert.equal(calls, 0);
});
