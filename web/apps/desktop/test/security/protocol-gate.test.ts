// say and answer pass the bridge only after archond's own welcome reported protocol 24 (Archon 1.5.0).

import assert from "node:assert/strict";
import { test } from "node:test";
import { Bridge, requestWithRefusal, type BridgeHost } from "../../src/bridge/bridge.ts";
import { BridgeRefusal } from "../../src/security/validate.ts";
import type { DaemonLink, Frame } from "../../src/bridge/daemon-link.ts";
import { APP_SENDER, VALIDATOR } from "./support.ts";

const OWNER = 1;
const DAEMON = { id: "archon:local", kind: "archon", label: "archond", socket: "/run/archond.sock" } as const;
const HELLO = { cmd: "hello", req: 1, protocol_version: 24, client: "archon-web", client_version: "0.0.0" };
const SAY = { cmd: "say", req: 2, text: "hello" };
const ANSWER = { cmd: "answer", req: 3, question: "q_1", text: "yes" };

test("a local refusal is a definite error reply and other failures remain visible", async () => {
  const refuse = async () => { throw new BridgeRefusal("invalid memory scope"); };
  assert.deepEqual(await requestWithRefusal(refuse, { req: 7 }), {
    frame: "reply", id: 7, result: { Err: { code: "forbidden", message: "invalid memory scope" } },
  });
  await assert.rejects(requestWithRefusal(refuse, { req: 0 }), /invalid memory scope/);
  await assert.rejects(requestWithRefusal(async () => { throw new Error("link lost"); }, { req: 7 }), /link lost/);
});

test("a closed connection remains a bridge failure instead of a forbidden reply", async () => {
  const bridge = bridgeToArchond(24, []);
  const conn = await open(bridge);
  await bridge.handle(APP_SENDER, OWNER, "close", [conn]);
  await assert.rejects(requestWithRefusal(() => send(bridge, conn, SAY), SAY), /not an open local connection/);
});

test("memory writes and export require native confirmation before the frame is sent", async () => {
  const sent: Frame[] = [];
  const shown: unknown[] = [];
  let allowed = false;
  const link = { isOpen: true, request: async (text: string) => { sent.push(JSON.parse(text) as Frame); return {}; },
    send: (text: string) => { sent.push(JSON.parse(text) as Frame); }, hasPending: () => false, close: () => {} } as unknown as DaemonLink;
  const host = { findDaemons: async () => [{ id: "aether:local", kind: "aether", label: "test", socket: "/private/aether.sock", reachable: true }],
    openLink: async () => link, confirm: async (_owner: number, confirmation: unknown) => { shown.push(confirmation); return allowed; } } as unknown as BridgeHost;
  const bridge = new Bridge(VALIDATOR, host);
  const conn = await bridge.handle(APP_SENDER, OWNER, "connect", ["aether:local"]) as string;
  for (const frame of [{ cmd: "consolidate_memories", dry_run: false }, { cmd: "memory_import", dry_run: false, artifact: "{}" }, { cmd: "memory_export" }]) {
    await assert.rejects(send(bridge, conn, { ...frame, req: 1 }), /Cancelled/);
    assert.equal(sent.length, 0);
    allowed = true;
    await send(bridge, conn, { ...frame, req: 2 });
    assert.deepEqual(sent.at(-1), { ...frame, req: 2 });
    allowed = false;
    sent.length = 0;
  }
  assert.equal(shown.length, 6);
  await assert.rejects(bridge.handle(APP_SENDER, OWNER, "subscribe", [conn, { cmd: "memory_import", dry_run: false, artifact: "{}", req: 4 }]), /Cancelled/);
  assert.equal(sent.length, 0);
  await send(bridge, conn, { cmd: "consolidate_memories", dry_run: true, req: 3 });
  assert.equal(shown.length, 7);
});

/** An archond that welcomes with `protocol` and answers anything else Ok. */
function bridgeToArchond(protocol: number | "garbled", sent: Frame[]): Bridge {
  const link = {
    isOpen: true,
    request: async (text: string) => {
      const frame = JSON.parse(text) as Frame;
      sent.push(frame);
      const data = frame.cmd === "hello" ? (protocol === "garbled" ? "ok" : { protocol_version: protocol }) : {};
      return { req: frame.req, result: { Ok: { data } } };
    },
    close: () => {},
  } as unknown as DaemonLink;
  const host = { findDaemons: async () => [{ ...DAEMON, reachable: true }], openLink: async () => link } as unknown as BridgeHost;
  return new Bridge(VALIDATOR, host);
}

async function open(bridge: Bridge): Promise<string> {
  return (await bridge.handle(APP_SENDER, OWNER, "connect", [DAEMON.id])) as string;
}

function send(bridge: Bridge, conn: string, frame: Frame): Promise<unknown> {
  return bridge.handle(APP_SENDER, OWNER, "request", [conn, frame]);
}

test("a protocol 23 welcome refuses say and answer and keeps the rest", async () => {
  const sent: Frame[] = [];
  const bridge = bridgeToArchond(23, sent);
  const conn = await open(bridge);
  await send(bridge, conn, { ...HELLO, protocol_version: 23 });
  for (const frame of [SAY, ANSWER]) await assert.rejects(send(bridge, conn, frame), /Archon 1\.5\.0 or newer/);
  await send(bridge, conn, { cmd: "status", req: 4 });
  assert.deepEqual(sent.map((frame) => frame.cmd), ["hello", "status"], "the refused frames never reach archond");
});

test("a protocol 24 welcome allows say and answer", async () => {
  const sent: Frame[] = [];
  const bridge = bridgeToArchond(24, sent);
  const conn = await open(bridge);
  await send(bridge, conn, HELLO);
  for (const frame of [SAY, ANSWER]) await send(bridge, conn, frame);
  assert.deepEqual(sent.map((frame) => frame.cmd), ["hello", "say", "answer"]);
});

test("before a welcome, and after a garbled one, the protocol is unknown and say is refused", async () => {
  for (const protocol of [24, "garbled"] as const) {
    const bridge = bridgeToArchond(protocol, []);
    const conn = await open(bridge);
    if (protocol === 24) await assert.rejects(send(bridge, conn, SAY), /unknown/);
    else {
      await send(bridge, conn, HELLO);
      await assert.rejects(send(bridge, conn, SAY), /unknown/);
    }
  }
});

test("the renderer cannot declare a protocol: a hello that archond refuses leaves it unknown", async () => {
  const bridge = bridgeToArchond(23, []);
  const conn = await open(bridge);
  await send(bridge, conn, { ...HELLO, protocol_version: 99 });
  await assert.rejects(send(bridge, conn, SAY), /protocol 23/);
});
