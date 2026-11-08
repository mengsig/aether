// An installed Archon 1.4.x refuses a protocol 24 hello with an id-0 error and closes. The link must
// hand that refusal to the waiting hello, so the app steps down to 23 instead of retrying 24 forever.

import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { test } from "node:test";
import { DaemonLink, type Frame, type LinkSocket } from "../../src/bridge/daemon-link.ts";

/** archond 1.4.12 over an in-memory socket: protocol 23 only, refusing anything else as it does. */
class Archond14 extends EventEmitter implements LinkSocket {
  closed = false;
  write(data: string): boolean {
    const frame = JSON.parse(data) as Frame;
    queueMicrotask(() => this.answer(frame));
    return true;
  }
  end(): void { this.close(); }
  destroy(): void { this.close(); }
  private answer(frame: Frame): void {
    if (frame.cmd === "hello" && frame.protocol_version === 23) {
      return this.line({ frame: "reply", id: frame.req, result: { Ok: { reply: "welcome", data: { protocol_version: 23, daemon_version: "1.4.12", pid: 7 } } } });
    }
    if (frame.cmd === "hello") {
      const message = `archond speaks protocol 23, this client test speaks ${String(frame.protocol_version)}; upgrade the older one`;
      this.line({ frame: "reply", id: 0, result: { Err: { code: "incompatible", message } } });
      return this.close();
    }
    this.line({ frame: "reply", id: 0, result: { Err: { code: "invalid_argument", message: "unexpected frame" } } });
  }
  private line(frame: Frame): void { this.emit("data", Buffer.from(`${JSON.stringify(frame)}\n`)); }
  private close(): void {
    if (this.closed) return;
    this.closed = true;
    this.emit("close");
  }
}

const hello = (req: number, version: number) => JSON.stringify({ req, cmd: "hello", protocol_version: version, client: "archon-web", client_version: "test" });

test("a 24 hello refused with an id-0 error settles as incompatible, and a new link at 23 is welcomed", async () => {
  const first = new DaemonLink(new Archond14());
  const stray: Frame[] = [];
  first.onFrame = (frame) => stray.push(frame);
  const refused = await first.request(hello(1, 24), 1);
  assert.equal(refused.id, 1, "the refusal is addressed to the hello it answers");
  assert.equal((refused.result as { Err?: { code?: string } }).Err?.code, "incompatible");
  assert.deepEqual(stray, [], "nothing is pushed as an unrequested frame");

  const second = new DaemonLink(new Archond14());
  const welcome = await second.request(hello(1, 23), 1);
  assert.equal((welcome.result as { Ok?: { data?: { protocol_version?: number } } }).Ok?.data?.protocol_version, 23);
});

test("after the handshake, an id-0 reply is never pinned on a waiting request", async () => {
  const socket = new Archond14();
  const link = new DaemonLink(socket);
  const stray: Frame[] = [];
  link.onFrame = (frame) => stray.push(frame);
  await link.request(hello(1, 23), 1);
  const late = link.request(JSON.stringify({ req: 2, cmd: "ping" }), 2, 50);
  await assert.rejects(late, /No reply to request 2 within 50 ms/);
  assert.equal(stray.length, 1, "the unaddressed error went to onFrame");
  assert.equal(stray[0]?.id, 0);
});
