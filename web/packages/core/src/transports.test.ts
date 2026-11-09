import type { DesktopBridge } from "@aether-ui/protocol";
import { describe, expect, it, vi } from "vitest";
import type { Json } from "./guards.ts";
import { newIdentity, type DeviceIdentity, type IdentityStore } from "./transports/identity.ts";
import { ipcTransport } from "./transports/ipc.ts";
import { Connection } from "./connection.ts";
import { INITIAL_STATE, Store } from "./store.ts";
import type { SocketLike } from "./transports/socket.ts";
import { aetherSocketTransport } from "./transports/ws-aether.ts";
import { archonSocketTransport } from "./transports/ws-archon.ts";

class FakeSocket implements SocketLike {
  readyState = 0;
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code: number; reason: string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  readonly sent: Json[] = [];
  closedWith: number | null = null;
  readonly url: string;
  readonly protocol: string;

  constructor(url: string, protocol: string) {
    this.url = url;
    this.protocol = protocol;
    queueMicrotask(() => { this.readyState = 1; this.onopen?.({}); });
  }

  send(data: string): void { this.sent.push(JSON.parse(data) as Json); }
  close(code = 1000): void { this.closedWith = code; this.readyState = 3; }
  serve(frame: Json): void { this.onmessage?.({ data: JSON.stringify(frame) }); }
}

function sockets() {
  const made: FakeSocket[] = [];
  const factory = (url: string, protocol: string) => {
    const socket = new FakeSocket(url, protocol);
    made.push(socket);
    return socket;
  };
  return { made, factory };
}

function memoryIdentity(initial: DeviceIdentity | undefined): IdentityStore & { saved: DeviceIdentity[] } {
  let current = initial;
  const saved: DeviceIdentity[] = [];
  return {
    saved,
    load: async () => current,
    save: async (identity) => { current = identity; saved.push(identity); },
    remove: async () => { current = undefined; },
  };
}

const NONCE = "n".repeat(43);
const ORIGIN = "https://host.example";
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function fromBase64url(text: string): Uint8Array<ArrayBuffer> {
  const binary = atob(text.replaceAll("-", "+").replaceAll("_", "/"));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}

describe("ws-aether", () => {
  it("cancels a socket awaiting its authentication challenge", async () => {
    const store = memoryIdentity({ ...(await newIdentity()), deviceId: "dev_1" });
    const { made, factory } = sockets();
    const controller = new AbortController();
    const opening = aetherSocketTransport({ identity: store, origin: ORIGIN, socket: factory }).open("wss://h/ws", () => {}, () => {}, controller.signal);
    await tick();
    const rejected = expect(opening).rejects.toMatchObject({ name: "AbortError" });
    controller.abort();
    await rejected;
    expect(made[0]?.closedWith).toBe(1000);
  });

  it("cancels a socket before it opens", async () => {
    const store = memoryIdentity({ ...(await newIdentity()), deviceId: "dev_1" });
    const { made, factory } = sockets();
    const controller = new AbortController();
    const opening = aetherSocketTransport({ identity: store, origin: ORIGIN, socket: factory }).open("wss://h/ws", () => {}, () => {}, controller.signal);
    await Promise.resolve();
    const rejected = expect(opening).rejects.toMatchObject({ name: "AbortError" });
    controller.abort();
    await rejected;
    expect(made[0]?.closedWith).toBe(1000);
  });

  it("signs the challenge into the hello and pins the daemon id on welcome", async () => {
    const identity = { ...(await newIdentity()), deviceId: "dev_1" };
    const store = memoryIdentity(identity);
    const { made, factory } = sockets();
    const frames: Json[] = [];
    const opening = aetherSocketTransport({ identity: store, origin: ORIGIN, socket: factory }).open("wss://host.example/ws", (f) => frames.push(f as Json), () => {});
    await tick();
    const socket = made[0];
    expect(socket?.protocol).toBe("aether");
    socket?.serve({ frame: "challenge", instance_id: "inst-a", nonce: NONCE });
    const link = await opening;
    link.send({ cmd: "hello", req: 1, protocol_version: 25 });
    const hello = socket?.sent[0] as { device: { id: string; signature: string } };
    expect(hello.device.id).toBe("dev_1");
    const message = new TextEncoder().encode(["aether-remote-auth-v1", "inst-a", ORIGIN, "dev_1", NONCE].join("\n"));
    const valid = await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, identity.keys.publicKey, fromBase64url(hello.device.signature), message);
    expect(valid).toBe(true);
    socket?.serve({ frame: "reply", id: 1, result: { Ok: { reply: "welcome", data: { instance_id: "inst-a", device: { id: "dev_1", name: "phone", role: "observer" } } } } });
    await tick();
    expect(store.saved).toEqual([expect.objectContaining({ daemonId: "inst-a", role: "observer", name: "phone" })]);
    expect(frames).toHaveLength(1);
    expect(() => link.send({ cmd: "hello", req: 2 })).toThrow(/once per link/);
  });

  it("refuses an unpaired browser before opening a socket", async () => {
    const { made, factory } = sockets();
    const opening = aetherSocketTransport({ identity: memoryIdentity(undefined), origin: ORIGIN, socket: factory }).open("wss://h/ws", () => {}, () => {});
    await expect(opening).rejects.toMatchObject({ code: "unpaired" });
    expect(made).toHaveLength(0);
  });

  it("reports an identity change when another daemon answers", async () => {
    const store = memoryIdentity({ ...(await newIdentity()), deviceId: "dev_1", daemonId: "inst-a" });
    const { made, factory } = sockets();
    const opening = aetherSocketTransport({ identity: store, origin: ORIGIN, socket: factory }).open("wss://h/ws", () => {}, () => {});
    await tick();
    made[0]?.serve({ frame: "challenge", instance_id: "inst-b", nonce: NONCE });
    await expect(opening).rejects.toMatchObject({ code: "identity_changed" });
    expect(made[0]?.closedWith).toBe(1000);
  });

  it("closes with a protocol error when the first frame is not a challenge", async () => {
    const store = memoryIdentity({ ...(await newIdentity()), deviceId: "dev_1" });
    const { made, factory } = sockets();
    const onClose = vi.fn();
    const opening = aetherSocketTransport({ identity: store, origin: ORIGIN, socket: factory }).open("wss://h/ws", () => {}, onClose);
    await tick();
    made[0]?.serve({ frame: "event", sub: 1 });
    await expect(opening).rejects.toThrow(/challenge/);
    expect(onClose).toHaveBeenCalledWith(1002, expect.stringContaining("challenge"));
  });

  it("refuses a socket outside /ws", async () => {
    const store = memoryIdentity({ ...(await newIdentity()), deviceId: "dev_1" });
    await expect(aetherSocketTransport({ identity: store, origin: ORIGIN }).open("https://h/ws", () => {}, () => {})).rejects.toThrow(/WebSocket URL/);
    await expect(aetherSocketTransport({ identity: store, origin: ORIGIN }).open("wss://h/other", () => {}, () => {})).rejects.toThrow(/must be at \/ws/);
  });
});

describe("ws-archon", () => {
  const answer = (status: number, body: Json = {}) => vi.fn(async () => new Response(JSON.stringify(body), { status }));

  it("asks for pairing when the session check answers 401", async () => {
    const { made, factory } = sockets();
    const fetcher = answer(401);
    await expect(archonSocketTransport({ socket: factory, fetch: fetcher }).open("wss://h/archon/ws", () => {}, () => {})).rejects.toMatchObject({ code: "unpaired" });
    expect(fetcher).toHaveBeenCalledWith("https://h/archon/api/session", expect.objectContaining({ credentials: "same-origin" }));
    expect(made).toHaveLength(0);
  });

  it("opens the socket under /archon once a session exists, or when archond predates the check", async () => {
    for (const fetcher of [answer(200, { device: { id: "d" } }), answer(404)]) {
      const { made, factory } = sockets();
      const frames: unknown[] = [];
      const link = await archonSocketTransport({ socket: factory, fetch: fetcher }).open("wss://h/archon/ws", (f) => frames.push(f), () => {});
      expect(made[0]).toMatchObject({ url: "wss://h/archon/ws", protocol: "archon" });
      link.send({ cmd: "hello", req: 1 });
      made[0]?.serve({ frame: "reply", id: 1 });
      expect(made[0]?.sent).toEqual([{ cmd: "hello", req: 1 }]);
      expect(frames).toEqual([{ frame: "reply", id: 1 }]);
    }
  });
});

describe("protocol errors on a socket", () => {
  it("closes with a code a browser may send and tells the app 1002", async () => {
    const { made, factory } = sockets();
    const onClose = vi.fn();
    await archonSocketTransport({ socket: factory, fetch: vi.fn(async () => Response.json({ device: {} })) }).open("wss://h/archon/ws", () => {}, onClose);
    made[0]?.onmessage?.({ data: "not json" });
    expect(made[0]?.closedWith).toBe(1000);
    expect(onClose).toHaveBeenCalledWith(1002, expect.stringContaining("Protocol error"));
  });
});

describe("ipc", () => {
  function bridge(overrides: Partial<DesktopBridge> = {}) {
    const unsubscribe = vi.fn();
    const desktop = {
      version: 1,
      connect: vi.fn(async () => "c1"),
      request: vi.fn(async (_conn: string, frame: unknown) => ({ frame: "reply", id: (frame as Json).req })),
      subscribe: vi.fn(async (_conn: string, frame: unknown, onFrame: (f: unknown) => void) => {
        onFrame({ frame: "reply", id: (frame as Json).req });
        return unsubscribe;
      }),
      close: vi.fn(async () => {}),
      onClose: vi.fn(() => () => {}),
      ...overrides,
    } as unknown as DesktopBridge;
    return { desktop, unsubscribe };
  }

  it("maps subscriptions to bridge.subscribe and other commands to bridge.request", async () => {
    const { desktop, unsubscribe } = bridge();
    const frames: unknown[] = [];
    const link = await ipcTransport(desktop, () => {}).open("aether-local", (f) => frames.push(f), () => {});
    link.send({ cmd: "hello", req: 1 });
    link.send({ cmd: "subscribe_changes", req: 2, after: null });
    await tick();
    expect(desktop.request).toHaveBeenCalledWith("c1", { cmd: "hello", req: 1 });
    expect(desktop.subscribe).toHaveBeenCalledWith("c1", { cmd: "subscribe_changes", req: 2, after: null }, expect.any(Function));
    expect(frames).toHaveLength(2);
    expect(frames).toEqual(expect.arrayContaining([{ frame: "reply", id: 1 }, { frame: "reply", id: 2 }]));
    link.close();
    expect(unsubscribe).toHaveBeenCalledTimes(1);
    expect(desktop.close).toHaveBeenCalledWith("c1");
  });

  it("closes the link with 1011 when a bridge call fails", async () => {
    const { desktop } = bridge({ request: vi.fn(async () => { throw new Error("main crashed"); }) });
    const onClose = vi.fn();
    const link = await ipcTransport(desktop, () => {}).open("aether-local", () => {}, onClose);
    link.send({ cmd: "status", req: 1 });
    await tick();
    expect(onClose).toHaveBeenCalledWith(1011, expect.stringContaining("main crashed"));
    expect(desktop.close).toHaveBeenCalledWith("c1");
    expect(() => link.send({ cmd: "status", req: 2 })).toThrow(/open/);
  });

  it("settles a refused request without closing the connection and permits a retry", async () => {
    const request = vi.fn().mockRejectedValueOnce(new Error("BridgeRefusal: denied"))
      .mockResolvedValueOnce({ frame: "reply", id: 2, result: { Ok: {} } });
    const { desktop } = bridge({ request });
    const frames: unknown[] = [];
    const onClose = vi.fn();
    const link = await ipcTransport(desktop, () => {}).open("archon-local", (frame) => frames.push(frame), onClose);
    link.send({ cmd: "get_initiative", req: 1 });
    await tick();
    expect(frames[0]).toMatchObject({ frame: "reply", id: 1, result: { Err: { code: "forbidden", message: expect.stringContaining("denied") } } });
    link.send({ cmd: "get_initiative", req: 2 });
    await tick();
    expect(frames[1]).toMatchObject({ frame: "reply", id: 2, result: { Ok: {} } });
    expect(onClose).not.toHaveBeenCalled();
    expect(desktop.close).not.toHaveBeenCalled();
  });

  it("reconnects when the bridge reports the local connection has closed", async () => {
    const { desktop } = bridge({ request: vi.fn(async () => { throw new Error("BridgeRefusal: c1 is closed"); }) });
    const onClose = vi.fn();
    const link = await ipcTransport(desktop, () => {}).open("archon-local", () => {}, onClose);
    link.send({ cmd: "get_initiative", req: 1 });
    await tick();
    expect(onClose).toHaveBeenCalledWith(1011, expect.stringContaining("is closed"));
    expect(desktop.close).toHaveBeenCalledWith("c1");
  });

  it("treats a missing local socket as a disconnected daemon, preserving other failures", async () => {
    const missing = bridge({ connect: vi.fn().mockRejectedValue(new Error("connect ENOENT /private/r.sock")) });
    await expect(ipcTransport(missing.desktop, () => {}).open("archon-local", () => {}, () => {}))
      .rejects.toMatchObject({ code: "disconnected", message: "The local daemon is not connected" });
    const denied = bridge({ connect: vi.fn().mockRejectedValue(new Error("access denied")) });
    await expect(ipcTransport(denied.desktop, () => {}).open("archon-local", () => {}, () => {})).rejects.toThrow("access denied");
  });

  it("moves to backoff and retries when main reports a closed link", async () => {
    vi.useFakeTimers();
    try {
      let notify: (() => void) | undefined;
      const { desktop } = bridge({
        onClose: vi.fn((_conn, callback) => { notify = callback; return () => { notify = undefined; }; }),
        request: vi.fn(() => new Promise(() => {})),
      });
      const store = new Store(INITIAL_STATE, (flush) => flush());
      const connection = new Connection({ daemon: "aether", transport: ipcTransport(desktop, () => {}), target: "local", label: "host" },
        { store, clientVersion: "test", random: () => 0, publish: () => {}, settingsChanged: () => {} });
      connection.start();
      await vi.advanceTimersByTimeAsync(0);
      expect(connection.current.state).toBe("connecting");
      expect(notify).toBeDefined();
      notify?.();
      expect(connection.current.state).toBe("backoff");
      await vi.advanceTimersByTimeAsync(250);
      expect(desktop.connect).toHaveBeenCalledTimes(2);
      connection.stop();
      await vi.advanceTimersByTimeAsync(1000);
      expect(desktop.connect).toHaveBeenCalledTimes(2);
      expect(connection.current.state).toBe("closed");
    } finally {
      vi.useRealTimers();
    }
  });

  it("refuses an unknown bridge version", () => {
    expect(() => ipcTransport({ version: 2 } as unknown as DesktopBridge, () => {})).toThrow(/version/);
  });
});
