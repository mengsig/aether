import { describe, expect, it, vi } from "vitest";
import { matchPattern, matchRoute, routeHref } from "./router.ts";
import { Registry } from "./registry.ts";
import { memoizedRead } from "./react.ts";
import { selectAgents } from "./selectors.ts";
import { dropConnection, EMPTY_AETHER, INITIAL_STATE, Store, type State } from "./store.ts";

function manualStore() {
  const frames: (() => void)[] = [];
  const store = new Store(INITIAL_STATE, (flush) => frames.push(flush));
  const runFrame = () => { for (const flush of frames.splice(0)) flush(); };
  return { store, frames, runFrame };
}

describe("Store", () => {
  it("batches any number of updates into one notification per frame", () => {
    const { store, frames, runFrame } = manualStore();
    const listener = vi.fn();
    store.subscribe(listener);
    for (let n = 1; n <= 50; n += 1) store.updateAether("aether:a", (s) => ({ ...s, lastSeq: n }));
    expect(frames).toHaveLength(1);
    expect(store.get().aether["aether:a"]?.lastSeq).toBe(50);
    expect(store.getSnapshot().aether["aether:a"]).toBeUndefined();
    runFrame();
    expect(listener).toHaveBeenCalledTimes(1);
    expect(store.getSnapshot()).toBe(store.get());
  });

  it("schedules nothing for an update that changes nothing", () => {
    const { store, frames } = manualStore();
    store.update((s) => s);
    store.updateAether("aether:a", (s) => s);
    expect(frames).toHaveLength(0);
    expect(store.get().aether["aether:a"]).toBeUndefined();
  });

  it("keeps the last 50 reports", () => {
    const { store } = manualStore();
    for (let n = 0; n < 60; n += 1) store.report(`r${n}`, "aether:a", "x");
    expect(store.get().reports).toHaveLength(50);
    expect(store.get().reports[0]?.message).toBe("r10");
  });

  it("carries a report's tone only when one is given", () => {
    const { store } = manualStore();
    store.report("Started Builder", "aether:a", undefined, "success");
    store.report("Could not refresh", "aether:a", "timeout");
    const [ok, failed] = store.get().reports;
    expect(ok).toMatchObject({ message: "Started Builder", tone: "success" });
    expect(failed).not.toHaveProperty("tone");
  });

  it("drops a connection's caches and moves its outbox to review as unknown", () => {
    const submission = { conn: "aether:a", key: "k", command: { cmd: "send_message" }, state: "pending" as const };
    const state: State = { ...INITIAL_STATE, aether: { "aether:a": { ...EMPTY_AETHER, lastSeq: 9, outbox: [submission] }, "aether:b": EMPTY_AETHER } };
    const next = dropConnection(state, "aether:a");
    expect(Object.keys(next.aether)).toEqual(["aether:b"]);
    expect(next.review).toEqual([{ ...submission, state: "unknown" }]);
  });
});

describe("selectors", () => {
  it("return stable references while their slice is unchanged", () => {
    const { store, runFrame } = manualStore();
    store.updateAether("aether:a", (s) => ({ ...s, agents: { a: { id: "a" } as never } }));
    runFrame();
    const read = memoizedRead(store, selectAgents("aether:a"), Object.is);
    const first = read();
    store.report("unrelated");
    runFrame();
    expect(read()).toBe(first);
    expect(Object.keys(first)).toEqual(["a"]);
  });
});

describe("hash router", () => {
  const load = async () => ({ default: null });
  const routes = [
    { path: "#/fleet/:view", owner: "a", load },
    { path: "#/fleet/topology", owner: "b", load },
    { path: "#/agents/:conn/:agent", owner: "c", load },
    { path: "#/files/*", owner: "d", load },
  ];

  it("prefers literal segments and decodes params", () => {
    expect(matchRoute(routes, "#/fleet/topology")?.route.owner).toBe("b");
    expect(matchRoute(routes, "#/fleet/list")).toMatchObject({ route: { owner: "a" }, params: { view: "list" } });
    expect(matchRoute(routes, "#/agents/aether%3Ainst-a/agt_1?tab=log")?.params).toEqual({ conn: "aether:inst-a", agent: "agt_1" });
    expect(matchRoute(routes, "#/files/a/b/c")?.params).toEqual({ "*": "a/b/c" });
    expect(matchRoute(routes, "#/agents/x")).toBeNull();
  });

  it("matches nothing for a broken escape instead of throwing", () => {
    expect(matchPattern("#/agents/:conn/:agent", "#/agents/%E0%A4/a")).toBeNull();
  });

  it("builds encoded hrefs that round-trip", () => {
    const href = routeHref("#/agents/:conn/:agent", { conn: "aether:inst-a", agent: "a/b" });
    expect(href).toBe("#/agents/aether%3Ainst-a/a%2Fb");
    expect(matchPattern("#/agents/:conn/:agent", href)).toEqual({ conn: "aether:inst-a", agent: "a/b" });
    expect(() => routeHref("#/agents/:conn")).toThrow(/needs :conn/);
  });
});

describe("Registry", () => {
  it("refuses bad and duplicate registrations and keeps identities stable", () => {
    const registry = new Registry();
    const load = async () => ({ default: null });
    const listener = vi.fn();
    registry.subscribe(listener);
    registry.registerRoutes([{ path: "#/queue", owner: "archon", load }]);
    const routes = registry.routes();
    expect(() => registry.registerRoutes([{ path: "#/queue", owner: "x", load }])).toThrow(/already registered/);
    expect(() => registry.registerRoutes([{ path: "/queue", owner: "x", load }])).toThrow(/must look like/);
    expect(registry.routes()).toBe(routes);
    registry.registerEntityAction({ id: "open", kind: "agent", label: "Open", run: () => {} } as never);
    expect(() => registry.registerEntityAction({ id: "open", kind: "agent", label: "Again" } as never)).toThrow(/already registered/);
    expect(() => registry.registerEntityAction({ id: "z", kind: "planet", label: "Z" } as never)).toThrow(/unknown entity kind/);
    expect(listener).toHaveBeenCalledTimes(2);
  });
});
