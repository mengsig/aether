import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { EventEmitter } from "node:events";
import { test } from "node:test";
import type { Session } from "electron";
import { MemoryExports } from "../../src/native/memory-exports.ts";
import { guardSession } from "../../src/security/navigation.ts";

const artifact = '{"memory":"example"}\n';
const digest = createHash("sha256").update(artifact).digest("hex");
const reply = { result: { Ok: { reply: "memory_export", data: { artifact, digest } } } };

test("only an intact export reply belongs to its originating window", () => {
  const exports = new MemoryExports();
  assert.throws(() => exports.get(1, digest), /No matching/);
  exports.remember(1, reply);
  assert.equal(exports.get(1, digest), artifact);
  assert.throws(() => exports.get(2, digest), /No matching/);
  assert.throws(() => exports.get(1, "0".repeat(64)), /No matching/);
  exports.remember(1, { result: { Ok: { reply: "other", data: { artifact, digest } } } });
  assert.throws(() => exports.get(1, digest), /No matching/);
  exports.remember(1, { result: { Ok: { reply: "memory_export", data: { artifact: "tampered", digest } } } });
  assert.throws(() => exports.get(1, digest), /No matching/);
  exports.remember(1, reply);
  exports.forget(1);
  assert.throws(() => exports.get(1, digest), /No matching/);
});

test("guarded sessions reject downloads regardless of source", () => {
  const session = Object.assign(new EventEmitter(), {
    setSpellCheckerEnabled: () => {}, setSpellCheckerLanguages: () => {},
    setPermissionRequestHandler: () => {}, setPermissionCheckHandler: () => {},
  });
  guardSession(session as unknown as Session);
  guardSession(session as unknown as Session);
  assert.equal(session.listenerCount("will-download"), 1);
  for (const source of ["https://example.com/file", "blob:app://aether/fake", "file:///tmp/file"]) {
    let prevented = false;
    session.emit("will-download", { preventDefault: () => { prevented = true; } }, { getURL: () => source });
    assert.equal(prevented, true, source);
  }
});

test("save IPC checks the caller, digest, and window before invoking the dialog", async () => {
  const { Bridge } = await import("../../src/bridge/bridge.ts");
  const { APP_SENDER, VALIDATOR } = await import("./support.ts");
  const saved: unknown[][] = [];
  let accepted = false;
  const host = { saveMemoryExport: async (...args: unknown[]) => { saved.push(args); return accepted; } };
  const bridge = new Bridge(VALIDATOR, host as never);
  bridge.memoryExports.remember(1, reply);
  for (const sender of [
    { ...APP_SENDER, isAppWindow: false }, { ...APP_SENDER, isTopFrame: false },
    { ...APP_SENDER, frameUrl: "https://example.com/" },
  ]) await assert.rejects(bridge.handle(sender, 1, "saveMemoryExport", [digest]));
  await assert.rejects(bridge.handle(APP_SENDER, 2, "saveMemoryExport", [digest]), /No matching/);
  await assert.rejects(bridge.handle(APP_SENDER, 1, "saveMemoryExport", ["../../file"]));
  assert.deepEqual(saved, []);
  assert.equal(await bridge.handle(APP_SENDER, 1, "saveMemoryExport", [digest]), false);
  accepted = true;
  assert.equal(await bridge.handle(APP_SENDER, 1, "saveMemoryExport", [digest]), true);
  assert.deepEqual(saved, Array(2).fill([1, `aether-memories-${digest.slice(0, 12)}.jsonl`, artifact]));
});

test("only a successful confirmed export request authorizes a save", async () => {
  const { Bridge } = await import("../../src/bridge/bridge.ts");
  const { APP_SENDER, VALIDATOR } = await import("./support.ts");
  let allowed = false;
  const saved: string[] = [];
  const link = { isOpen: true, close: () => {}, request: async () => reply };
  const host = {
    findDaemons: async () => [{ id: "aether:local", kind: "aether", label: "test", socket: "/tmp/aether.sock", reachable: true }],
    openLink: async () => link, confirm: async () => allowed,
    saveMemoryExport: async (_owner: number, _name: string, contents: string) => { saved.push(contents); return true; },
  };
  const bridge = new Bridge(VALIDATOR, host as never);
  const conn = await bridge.handle(APP_SENDER, 1, "connect", ["aether:local"]);
  const request = () => bridge.handle(APP_SENDER, 1, "request", [conn, { cmd: "memory_export", req: 1 }]);
  await assert.rejects(request(), /Cancelled/);
  await assert.rejects(bridge.handle(APP_SENDER, 1, "saveMemoryExport", [digest]), /No matching/);
  allowed = true;
  await request();
  assert.equal(await bridge.handle(APP_SENDER, 1, "saveMemoryExport", [digest]), true);
  assert.deepEqual(saved, [artifact]);
});
