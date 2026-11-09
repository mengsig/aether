import { afterEach, describe, expect, it, vi } from "vitest";
import type { Transport } from "@aether-ui/protocol";
import { Connection, CONNECT_TIMEOUT_MS } from "./connection.ts";
import { Store, INITIAL_STATE } from "./store.ts";
import { archonSocketTransport } from "./transports/ws-archon.ts";
import type { SocketLike } from "./transports/socket.ts";

const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
};
function connection(transport: Transport) {
  return new Connection({ daemon: "archon", target: "ws://localhost/archon/ws", label: "local", transport }, {
    store: new Store(INITIAL_STATE, (f) => f()), clientVersion: "test", publish: () => {}, settingsChanged: () => {}, random: () => 0,
  });
}
afterEach(() => { vi.useRealTimers(); });

describe("connection recovery", () => {
  it("times out an ignored signal, backs off and closes a late link", async () => {
    vi.useFakeTimers();
    const pending = deferred<{ send(): void; close(): void }>();
    const close = vi.fn();
    const transport: Transport = { id: "ipc", open: vi.fn(() => pending.promise) };
    const c = connection(transport);
    c.start();
    await vi.advanceTimersByTimeAsync(CONNECT_TIMEOUT_MS);
    expect(c.current.state).toBe("backoff");
    pending.resolve({ send: () => {}, close });
    await flush();
    expect(close).toHaveBeenCalledOnce();
    c.stop();
  });

  it("aborts a connecting attempt and starts exactly one replacement", async () => {
    const first = deferred<{ send(): void; close(): void }>();
    const close = vi.fn();
    const second = deferred<{ send(): void; close(): void }>();
    const signals: AbortSignal[] = [];
    const transport: Transport = { id: "ipc", open: vi.fn((_t, _f, _c, signal) => {
      if (!signal) throw new Error("Expected a cancellation signal");
      signals.push(signal); return signals.length === 1 ? first.promise : second.promise;
    }) };
    const c = connection(transport);
    c.start(); c.retryNow();
    expect(signals).toHaveLength(2);
    expect(signals[0]?.aborted).toBe(true);
    first.resolve({ send: () => {}, close });
    await flush();
    expect(close).toHaveBeenCalledOnce();
    c.stop();
    expect(signals[1]?.aborted).toBe(true);
  });

  it.each(["fetch", "body"])("bounds a hung session %s", async (stage) => {
    vi.useFakeTimers();
    const never = new Promise<never>(() => {});
    const fetcher = vi.fn(() => stage === "fetch" ? never : Promise.resolve({ ok: true, status: 200, json: () => never } as unknown as Response));
    const c = connection(archonSocketTransport({ fetch: fetcher as typeof fetch }));
    c.start();
    await vi.advanceTimersByTimeAsync(CONNECT_TIMEOUT_MS);
    expect(c.current.state).toBe("backoff");
    c.stop();
  });

  it("closes a socket still opening on cancellation", async () => {
    const socket: SocketLike = { readyState: 0, onopen: null, onclose: null, onmessage: null, onerror: null, send: () => {}, close: vi.fn() };
    const fetcher = vi.fn(async () => ({ status: 404 } as Response));
    const c = connection(archonSocketTransport({ fetch: fetcher as typeof fetch, socket: () => socket }));
    c.start(); await flush(); c.stop();
    expect(socket.close).toHaveBeenCalledOnce();
    expect(socket.onopen).toBeNull();
  });

  it("times out and closes a WebSocket that never opens", async () => {
    vi.useFakeTimers();
    const socket: SocketLike = { readyState: 0, onopen: null, onclose: null, onmessage: null, onerror: null, send: () => {}, close: vi.fn() };
    const fetcher = vi.fn(async () => ({ status: 404 } as Response));
    const c = connection(archonSocketTransport({ fetch: fetcher as typeof fetch, socket: () => socket }));
    c.start(); await flush();
    await vi.advanceTimersByTimeAsync(CONNECT_TIMEOUT_MS);
    expect(c.current.state).toBe("backoff");
    expect(socket.close).toHaveBeenCalledOnce();
    c.stop();
  });

  it.each([[401, "pairing"], [404, "connecting"]] as const)("keeps session %s compatibility", async (status, state) => {
    const socket: SocketLike = { readyState: 0, onopen: null, onclose: null, onmessage: null, onerror: null, send: () => {}, close: vi.fn() };
    const fetcher = vi.fn(async () => ({ status } as Response));
    const c = connection(archonSocketTransport({ fetch: fetcher as typeof fetch, socket: () => socket }));
    c.start(); await flush();
    expect(c.current.state).toBe(state);
    if (status === 404) expect(socket.onopen).not.toBeNull();
    c.stop();
  });
});
