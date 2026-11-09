import { describe, expect, it, vi } from "vitest";
import type { DesktopBridge } from "@aether-ui/protocol";
import { ipcTransport } from "./transports/ipc.ts";
import { invariant } from "./guards.ts";

function required<T>(value: T | undefined): T {
  invariant(value !== undefined, "test fixture exists");
  return value;
}

function setup() {
  const callbacks: Array<(frame: unknown) => void> = [];
  const releases: Array<() => void> = [];
  const resolves: Array<(release: () => void) => void> = [];
  const requests = new Map<number, (frame: unknown) => void>();
  let closed = () => {};
  const desktop = {
    version: 1, connect: async () => "c", close: vi.fn(async () => {}),
    onClose: (_conn: string, callback: () => void) => { closed = callback; return vi.fn(); },
    subscribe: (_conn: string, _frame: unknown, callback: (frame: unknown) => void) => {
      callbacks.push(callback);
      releases.push(vi.fn());
      return new Promise<() => void>((resolve) => resolves.push(resolve));
    },
    request: (_conn: string, frame: { req: number }) => new Promise((resolve) => requests.set(frame.req, resolve)),
  } as unknown as DesktopBridge;
  const frames: unknown[] = [];
  const onClose = vi.fn();
  return { desktop, callbacks, releases, resolves, requests, frames, onClose, closed: () => closed() };
}

const reply = (id: number, result: unknown) => ({ frame: "reply", id, result });
const subscribed = (id: number) => ({ Ok: { reply: "subscribed", data: { id } } });
const ok = { Ok: { reply: "ok", data: null } };
const err = { Err: { code: "denied", message: "no" } };
const tick = async () => { await Promise.resolve(); await Promise.resolve(); };

describe("IPC subscription releases", () => {
  it("releases only the unsubscribed agent through repeated switches", async () => {
    const s = setup();
    const link = await ipcTransport(s.desktop, vi.fn()).open("local", (f) => s.frames.push(f), s.onClose);
    link.send({ cmd: "subscribe", req: 1 });
    link.send({ cmd: "subscribe", req: 2 });
    required(s.callbacks[0])(reply(2, subscribed(20)));
    required(s.callbacks[0])(reply(1, subscribed(10)));
    required(s.resolves[0])(required(s.releases[0])); required(s.resolves[1])(required(s.releases[1]));
    await tick();
    for (let i = 0; i < 3; i++) {
      const req = 3 + i * 2;
      link.send({ cmd: "unsubscribe", req, id: 10 + i });
      required(s.requests.get(req))(reply(req, ok));
      await tick();
      expect(s.releases[i === 0 ? 0 : i + 1]).toHaveBeenCalledTimes(1);
      expect(s.releases[1]).not.toHaveBeenCalled();
      link.send({ cmd: "subscribe", req: req + 1 });
      required(s.callbacks[0])(reply(req + 1, subscribed(11 + i)));
      required(s.resolves[i + 2])(required(s.releases[i + 2]));
      await tick();
    }
    link.close();
    for (const release of s.releases) expect(release).toHaveBeenCalledTimes(1);
    expect(s.frames).toHaveLength(8);
  });

  it("handles unsubscribe and close before the subscribe promise resolves", async () => {
    const s = setup();
    const link = await ipcTransport(s.desktop, vi.fn()).open("local", (f) => s.frames.push(f), s.onClose);
    link.send({ cmd: "subscribe", req: 1 });
    required(s.callbacks[0])(reply(1, subscribed(10)));
    link.send({ cmd: "unsubscribe", req: 2, id: 10 });
    required(s.requests.get(2))(reply(2, ok));
    await tick();
    required(s.resolves[0])(required(s.releases[0]));
    await tick();
    expect(s.releases[0]).toHaveBeenCalledTimes(1);
    link.send({ cmd: "subscribe_feed", req: 3 });
    s.closed();
    required(s.resolves[1])(required(s.releases[1]));
    await tick();
    expect(s.releases[1]).toHaveBeenCalledTimes(1);
    expect(s.onClose).toHaveBeenCalledTimes(1);
    expect(s.frames).toHaveLength(2);
  });

  it("retains a live subscription after failed or mismatched unsubscribe and releases failed subscriptions", async () => {
    const s = setup();
    const link = await ipcTransport(s.desktop, vi.fn()).open("local", (f) => s.frames.push(f), s.onClose);
    link.send({ cmd: "subscribe_changes", req: 1 });
    link.send({ cmd: "subscribe_feed", req: 2 });
    required(s.callbacks[0])(reply(1, subscribed(10)));
    required(s.callbacks[0])(reply(2, err));
    required(s.resolves[0])(required(s.releases[0])); required(s.resolves[1])(required(s.releases[1]));
    await tick();
    expect(s.releases[1]).toHaveBeenCalledTimes(1);
    link.send({ cmd: "unsubscribe", req: 3, id: 10 });
    required(s.requests.get(3))(reply(3, err));
    link.send({ cmd: "unsubscribe", req: 4, id: 10 });
    required(s.requests.get(4))(reply(999, ok));
    await tick();
    expect(s.releases[0]).not.toHaveBeenCalled();
    expect(s.frames).toEqual([reply(1, subscribed(10)), reply(2, err), reply(3, err), reply(999, ok)]);
    link.close();
    expect(s.releases[0]).toHaveBeenCalledTimes(1);
  });

  it("does not release a later subscription when an old unsubscribe reply arrives late", async () => {
    const s = setup();
    const link = await ipcTransport(s.desktop, vi.fn()).open("local", () => {}, s.onClose);
    link.send({ cmd: "subscribe", req: 1 });
    required(s.callbacks[0])(reply(1, subscribed(10)));
    required(s.resolves[0])(required(s.releases[0]));
    link.send({ cmd: "unsubscribe", req: 2, id: 10 });
    link.send({ cmd: "unsubscribe", req: 3, id: 10 });
    required(s.requests.get(3))(reply(3, ok));
    await tick();
    link.send({ cmd: "subscribe", req: 4 });
    required(s.callbacks[0])(reply(4, subscribed(10)));
    required(s.resolves[1])(required(s.releases[1]));
    required(s.requests.get(2))(reply(2, ok));
    await tick();
    expect(s.releases[0]).toHaveBeenCalledTimes(1);
    expect(s.releases[1]).not.toHaveBeenCalled();
    link.close();
    expect(s.releases[1]).toHaveBeenCalledTimes(1);
  });

  it("delivers bridge refusals without closing or releasing other subscriptions", async () => {
    const s = setup();
    const subscribe = s.desktop.subscribe;
    s.desktop.subscribe = vi.fn().mockImplementationOnce(subscribe)
      .mockRejectedValueOnce(new Error("BridgeRefusal: denied"));
    s.desktop.request = vi.fn().mockRejectedValue(new Error("BridgeRefusal: denied"));
    const link = await ipcTransport(s.desktop, vi.fn()).open("local", (f) => s.frames.push(f), s.onClose);
    link.send({ cmd: "subscribe", req: 1 });
    required(s.callbacks[0])(reply(1, subscribed(10)));
    required(s.resolves[0])(required(s.releases[0]));
    link.send({ cmd: "subscribe", req: 2 });
    link.send({ cmd: "unsubscribe", req: 3, id: 10 });
    await tick();
    expect(s.frames).toEqual([reply(1, subscribed(10)),
      reply(2, { Err: { code: "forbidden", message: "denied" } }),
      reply(3, { Err: { code: "forbidden", message: "denied" } })]);
    expect(s.releases[0]).not.toHaveBeenCalled();
    expect(s.onClose).not.toHaveBeenCalled();
    link.close();
    expect(s.releases[0]).toHaveBeenCalledTimes(1);
  });
});
