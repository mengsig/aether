// The one validation point refuses, before anything runs: oversize input, unknown methods and
// commands, host-only and remote-admin commands, shutdown, foreign senders and bad actions.

import assert from "node:assert/strict";
import { test } from "node:test";
import { BridgeRefusal } from "../../src/security/validate.ts";
import { AETHER_CONN, APP_SENDER, ARCHON_CONN, CONNECTIONS, MEMORY, VALIDATOR } from "./support.ts";

const check = (method: string, args: unknown[], sender = APP_SENDER) => VALIDATOR.check(sender, method, args, CONNECTIONS);

function refused(method: string, args: unknown[], pattern: RegExp, sender = APP_SENDER): void {
  assert.throws(() => check(method, args, sender), (error: unknown) => {
    assert.ok(error instanceof BridgeRefusal, `a BridgeRefusal, not ${String(error)}`);
    assert.match(error.message, pattern);
    return true;
  });
}

test("a status request passes with its exact text", () => {
  const frame = { cmd: "status", req: 4 };
  const call = check("request", [AETHER_CONN, frame]);
  assert.deepEqual(call, { method: "request", conn: AETHER_CONN, frame, text: JSON.stringify(frame) });
  assert.equal(check("subscribe", [ARCHON_CONN, { cmd: "subscribe_changes", req: 5 }]).method, "subscribe");
});

test("notifications accept only a boolean background-only preference", () => {
  assert.equal(check("notify", [{ title: "Question", body: "Ready?", backgroundOnly: true }]).method, "notify");
  refused("notify", [{ title: "Question", body: "Ready?", backgroundOnly: "true" }], /boolean/);
  refused("notify", [{ title: "Question", body: "Ready?", backgroundOnly: null }], /boolean/);
});

test("unknown methods and commands are refused", () => {
  refused("eval", [], /Unknown bridge method/);
  refused("request", [AETHER_CONN, { cmd: "no_such_command", req: 1 }], /no_such_command|cmd/);
  refused("request", [ARCHON_CONN, { cmd: "no_such_command", req: 1 }], /no_such_command|cmd/);
  refused("request", [AETHER_CONN, { cmd: "status" }], /req/);
  refused("connect", ["aether:local", "extra"], /connect/);
});

test("oversize frames and strings are refused", () => {
  const big = "x".repeat(1024 * 1024);
  refused("request", [AETHER_CONN, { cmd: "send_message", id: "a", text: big, req: 1 }], /1 MiB/);
  refused("openExternal", [`https://example.com/${"a".repeat(5000)}`], /4 KiB/);
  refused("setPreference", ["layout.panels", { deep: "b".repeat(4097) }], /4 KiB/);
});

test("host-only changes, remote-admin changes and device-self commands never pass through request", () => {
  for (const frame of [
    { cmd: "accept_memory", id: MEMORY }, { cmd: "reject_memory", id: MEMORY, reason: "x" },
    { cmd: "storage_gc" }, { cmd: "configure_remote", expected_revision: 1, settings: { enabled: true } }, { cmd: "start_pairing", role: "operator" },
    { cmd: "revoke_device", id: "dev_x" }, { cmd: "test_push" },
    { cmd: "add_approval_rule", scope: { scope: "everywhere" }, tool: "bash", decision: "allow" },
    { cmd: "list_devices", include_revoked: true },
    { cmd: "noesis_configure", patch: { enabled: true } },
  ]) {
    assert.throws(() => check("request", [AETHER_CONN, { ...frame, req: 1 }]), BridgeRefusal, frame.cmd);
    assert.throws(() => check("subscribe", [AETHER_CONN, { ...frame, req: 1 }]), BridgeRefusal, frame.cmd);
  }
});

test("shutdown is refused through every method", () => {
  refused("request", [AETHER_CONN, { cmd: "shutdown", req: 1 }], /shutdown/);
  refused("subscribe", [AETHER_CONN, { cmd: "shutdown", req: 1 }], /shutdown/);
  refused("request", [ARCHON_CONN, { cmd: "shutdown", req: 1 }], /shutdown/);
  refused("privileged", [{ kind: "shutdown", conn: AETHER_CONN }], /Not a privileged action/);
});

test("archon commands outside the pass list are refused", () => {
  refused("request", [ARCHON_CONN, { cmd: "pause", scope: "global", hard: true, req: 1 }], /hard/);
  for (const cmd of ["answer", "say", "merge_item", "set_settings"]) {
    assert.throws(() => check("request", [ARCHON_CONN, { cmd, req: 1 }]), BridgeRefusal, cmd);
  }
});

test("calls from anything but the top app:// frame of an app window are refused", () => {
  const frame = [AETHER_CONN, { cmd: "status", req: 1 }];
  refused("request", frame, /Only the app window/, { ...APP_SENDER, isAppWindow: false });
  refused("request", frame, /subframe/, { ...APP_SENDER, isTopFrame: false });
  refused("discover", [], /top-level app:/, { ...APP_SENDER, frameUrl: "https://evil.example/" });
  refused("discover", [], /top-level app:/, { ...APP_SENDER, frameUrl: "app://other/index.html" });
  refused("discover", [], /top-level app:/, { ...APP_SENDER, frameUrl: undefined });
});

test("privileged actions outside the union, or on the wrong connection, are refused", () => {
  refused("privileged", [{ kind: "memory.forget", conn: AETHER_CONN, memory: MEMORY }], /Not a privileged action/);
  refused("privileged", [{ kind: "memory.accept", conn: AETHER_CONN, memory: MEMORY, cmd: "shutdown" }], /Not a privileged/);
  refused("privileged", [{ kind: "memory.accept", conn: AETHER_CONN, memory: "mem_../x" }], /Not a privileged/);
  refused("privileged", [{ kind: "memory.accept", conn: "remote:dev_1", memory: MEMORY }], /Not a privileged/);
  refused("privileged", [{ kind: "memory.accept", conn: "local:00000000-0000-4000-8000-00000000000f", memory: MEMORY }],
    /local connection/);
  refused("privileged", [{ kind: "memory.accept", conn: ARCHON_CONN, memory: MEMORY }], /aether connection/);
  const action = { kind: "memory.accept", conn: AETHER_CONN, memory: MEMORY };
  assert.deepEqual(check("privileged", [action]), { method: "privileged", action });
});

test("frames on a connection that is not open, or remote, are refused", () => {
  refused("request", ["remote:dev_1", { cmd: "status", req: 1 }], /request/);
  refused("request", ["local:00000000-0000-4000-8000-00000000000f", { cmd: "status", req: 1 }], /not an open local/);
});

test("values JSON would change are refused, so what is confirmed is what is sent", () => {
  for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
    const action = { kind: "archon.operator_change", conn: ARCHON_CONN, request: "settings", patch: { budget_usd_daily: bad } };
    refused("privileged", [action], /finite/);
    refused("request", [AETHER_CONN, { cmd: "status", req: 1, extra: bad }], /finite/);
    refused("setPreference", ["window.bounds", { x: bad }], /finite/);
  }
  const call = check("request", [AETHER_CONN, { cmd: "status", req: 2, unset: undefined }]);
  assert.ok(call.method === "request" && call.text === JSON.stringify({ cmd: "status", req: 2 }));
  let deep: unknown = 1;
  for (let i = 0; i < 300; i += 1) deep = [deep];
  refused("request", [AETHER_CONN, { cmd: "status", req: 3, deep }], /nests too deeply/);
});


test("get_remote passes only as an exact request on an open local Aether connection", () => {
  const frame = { cmd: "get_remote", req: 8 };
  assert.deepEqual(check("request", [AETHER_CONN, frame]), { method: "request", conn: AETHER_CONN, frame, text: JSON.stringify(frame) });
  refused("subscribe", [AETHER_CONN, frame], /only through request/);
  refused("request", [ARCHON_CONN, frame], /get_remote|cmd/);
  refused("request", ["remote:dev_1", frame], /request/);
  refused("request", ["local:00000000-0000-4000-8000-00000000000f", frame], /not an open local/);
  for (const req of [undefined, null, "8", 0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
    refused("request", [AETHER_CONN, { cmd: "get_remote", req }], /req/);
  }
  for (const extra of [{ settings: { enabled: true } }, { caller: "agt_1" }, { extra: null }]) {
    refused("request", [AETHER_CONN, { ...frame, ...extra }], /only cmd and req|additional/);
  }
});
