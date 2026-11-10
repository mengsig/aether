import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApp } from "./app.ts";

const storage = { get: async () => undefined, set: async () => {} };
const identities = { load: async () => undefined, save: async () => {}, remove: async () => {} };
const health = (mounted: boolean) => ({ ok: true, status: 200, json: async () => ({ mounts: mounted ? [{ prefix: "/archon", kind: "archon" }] : [] }) } as Response);
const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
let events: EventTarget;
beforeEach(() => {
  vi.useFakeTimers();
  events = new EventTarget();
  vi.stubGlobal("location", new URL("https://example.test/"));
  vi.stubGlobal("addEventListener", events.addEventListener.bind(events));
  vi.stubGlobal("removeEventListener", events.removeEventListener.bind(events));
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe("browser discovery recovery", () => {
  it.each([true, false])("clears only discovery failures after automatic health recovery (mounted: %s)", async (mounted) => {
    let healthCalls = 0;
    const fetcher = vi.fn(async (url: RequestInfo | URL) => {
      if (String(url).includes("/archon/api/session")) return { ok: false, status: 401 } as Response;
      if (++healthCalls === 1) throw new Error("health refused");
      return health(mounted);
    });
    const app = createApp({ kind: "browser", storage }, { fetch: fetcher as typeof fetch, identities, scheduler: (f) => f() });
    await app.start(); await flush();
    expect(app.manager.all()[0]?.current.state).toBe("pairing");
    expect(app.store.get().reports.map((r) => r.message)).toContain("Could not discover Archon: health refused");
    app.store.report("Unrelated failure");
    app.store.report("Could not discover Archon: health refused", "another-connection");
    await vi.advanceTimersByTimeAsync(500);
    expect(healthCalls).toBe(2);
    expect(app.store.get().reports.map((r) => r.message)).toEqual(["Unrelated failure", "Could not discover Archon: health refused"]);
    expect(app.manager.all()[0]?.current.state).toBe("pairing");
    expect(app.manager.all().map((c) => c.endpoint.daemon)).toEqual(mounted ? ["aether", "archon"] : ["aether"]);
    app.stop();
  });

  it("keeps a discovery failure when a late successful response arrives after stop", async () => {
    let resolve!: (response: Response) => void;
    const fetcher = vi.fn().mockRejectedValueOnce(new Error("offline"))
      .mockImplementationOnce(() => new Promise<Response>((done) => { resolve = done; }));
    const app = createApp({ kind: "browser", storage }, { fetch: fetcher as typeof fetch, identities, scheduler: (f) => f() });
    await app.start(); await flush();
    await vi.advanceTimersByTimeAsync(500);
    expect(fetcher).toHaveBeenCalledTimes(2);
    app.stop();
    const stopped = app.store.get();
    resolve(health(true)); await flush();
    expect(app.store.get()).toBe(stopped);
    expect(stopped.reports.map((r) => r.message)).toContain("Could not discover Archon: offline");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fetcher).toHaveBeenCalledTimes(2);
  });

  it("restarts existing Aether and discovered Archon connections after stop", async () => {
    const fetcher = vi.fn(async (url: RequestInfo | URL) => {
      if (String(url).includes("/api/health")) return health(true);
      return new Promise<Response>(() => {});
    });
    const app = createApp({ kind: "browser", storage }, { fetch: fetcher as typeof fetch, identities, scheduler: (f) => f() });
    await app.start(); await flush();
    const connections = [...app.manager.all()];
    expect(connections.map((c) => c.endpoint.daemon)).toEqual(["aether", "archon"]);
    app.stop();
    expect(connections.map((c) => c.current.state)).toEqual(["closed", "closed"]);
    const starts = connections.map((c) => vi.spyOn(c, "start"));
    await app.start(); await flush();
    expect(app.manager.all()).toEqual(connections);
    expect(starts.map((start) => start.mock.calls.length)).toEqual([1, 1]);
    expect(connections.every((c) => c.current.state !== "closed")).toBe(true);
    app.stop();
  });

  it.each(["fetch", "body"])("starts Aether while health %s hangs and retries", async (stage) => {
    const never = new Promise<never>(() => {});
    const fetcher = vi.fn(() => stage === "fetch" ? never : Promise.resolve({ ok: true, json: () => never } as unknown as Response));
    const app = createApp({ kind: "browser", storage }, { fetch: fetcher as typeof fetch, identities, scheduler: (f) => f() });
    await app.start(); await flush();
    expect(app.manager.all().map((c) => c.endpoint.daemon)).toEqual(["aether"]);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(app.store.get().reports.some((r) => r.message.includes("discover Archon"))).toBe(true);
    await vi.advanceTimersByTimeAsync(1000);
    expect(fetcher.mock.calls.length).toBeGreaterThan(1);
    app.stop();
    const calls = fetcher.mock.calls.length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fetcher).toHaveBeenCalledTimes(calls);
  });

  it("recovers a failed startup through online and deduplicates rediscovery", async () => {
    let mounted = false;
    const fetcher = vi.fn(async () => { if (!mounted) throw new Error("offline"); return health(true); });
    const app = createApp({ kind: "browser", storage }, { fetch: fetcher as typeof fetch, identities, scheduler: (f) => f() });
    await app.start(); await flush();
    expect(app.manager.all()).toHaveLength(1);
    mounted = true;
    events.dispatchEvent(new Event("online")); await flush();
    expect(app.manager.all().map((c) => c.endpoint.daemon)).toEqual(["aether", "archon"]);
    await expect(app.rediscover()).resolves.toBe(0);
    app.stop();
    const calls = fetcher.mock.calls.length;
    events.dispatchEvent(new Event("online")); await flush();
    expect(fetcher).toHaveBeenCalledTimes(calls);
  });

  it("adds a mount on explicit rediscovery and ignores a late health response after stop", async () => {
    let resolve!: (response: Response) => void;
    let pending = false;
    const fetcher = vi.fn(() => pending ? new Promise<Response>((done) => { resolve = done; }) : Promise.resolve(health(false)));
    const app = createApp({ kind: "browser", storage }, { fetch: fetcher as typeof fetch, identities, scheduler: (f) => f() });
    await app.start(); await flush();
    pending = true;
    const discovery = app.rediscover();
    resolve(health(true));
    await expect(discovery).resolves.toBe(1);
    const later = app.rediscover();
    app.stop();
    resolve(health(true));
    await expect(later).resolves.toBe(0);
    expect(app.manager.all()).toHaveLength(2);
  });

  it("discovers a later mount on explicit rediscovery and after pairing", async () => {
    let mounted = false;
    const fetcher = vi.fn(async (url: RequestInfo | URL) => {
      if (String(url).includes("pair/claim")) return { ok: true, status: 200, text: async () => JSON.stringify({ device: { id: "device" } }) } as Response;
      return health(mounted);
    });
    const app = createApp({ kind: "browser", storage }, { fetch: fetcher as typeof fetch, identities, scheduler: (f) => f() });
    await app.start(); await flush();
    await expect(app.rediscover()).resolves.toBe(0);
    mounted = true;
    await app.pairing("archon").start("code", "browser"); await flush();
    expect(app.manager.all().map((c) => c.endpoint.daemon)).toEqual(["aether", "archon"]);
    expect(await app.rediscover()).toBe(0);
    app.stop();
  });
});
