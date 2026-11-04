import type { GraphModel } from "@aether-ui/protocol";
import { describe, expect, it } from "vitest";
import { positionBytes, seededGraph } from "../test/graphs.ts";
import { LayoutEngine, type LayoutWorker } from "./engine.ts";
import { runRequest, type LayoutReply, type LayoutRequest } from "./worker-protocol.ts";

/** An in-process worker that answers asynchronously, like a real one; `tamper` edits replies. */
function fakeWorker(tamper: (reply: LayoutReply) => LayoutReply = (r) => r) {
  return (): LayoutWorker => {
    let reply: (r: LayoutReply) => void = () => undefined;
    return {
      postMessage: (request: LayoutRequest) => queueMicrotask(() => reply(tamper(runRequest(request)))),
      onReply: (listener) => { reply = listener; },
      onFailure: () => undefined,
      terminate: () => undefined,
    };
  };
}

const model = seededGraph(60, 120);

describe("LayoutEngine", () => {
  it("does not relayout on a status-only change", async () => {
    const engine = new LayoutEngine({ createWorker: fakeWorker() });
    const first = await engine.layout(model, "layered");
    const restatused: GraphModel = { ...model, nodes: model.nodes.map((n) => ({ ...n, status: "failed", label: `${n.label}!` })) };
    const second = await engine.layout(restatused, "layered");
    expect(engine.stats).toEqual({ computed: 1, cacheHits: 1 });
    expect(second).toBe(first);
    await engine.layout({ ...model, edges: model.edges.slice(1) }, "layered");
    expect(engine.stats.computed).toBe(2);
  });

  it("keys layouts by structure hash and algorithm, with byte-identical results across engines", async () => {
    const one = new LayoutEngine({ createWorker: fakeWorker() });
    const two = new LayoutEngine({ createWorker: fakeWorker() });
    const a = await one.layout(model, "layered");
    const b = await two.layout({ ...model, nodes: [...model.nodes].reverse() }, "layered");
    expect(b.hash).toBe(a.hash);
    expect(positionBytes(b.positions)).toBe(positionBytes(a.positions));
    const f = await one.layout(model, "force");
    expect(one.stats.computed).toBe(2);
    expect(positionBytes(f.positions)).not.toBe(positionBytes(a.positions));
  });

  it("relays out when a group moves under another, matching a fresh engine", async () => {
    const roots: GraphModel = {
      nodes: [{ id: "a", label: "A", group: "g" }, { id: "b", label: "B", group: "h" }, { id: "c", label: "C", group: "k" }],
      edges: [], groups: [{ id: "g", label: "G" }, { id: "h", label: "H" }, { id: "k", label: "K" }], collapsed: new Set(),
    };
    const nested: GraphModel = { ...roots, groups: [{ id: "g", label: "G" }, { id: "h", label: "H" }, { id: "k", label: "K", parent: "g" }] };
    const engine = new LayoutEngine({ createWorker: fakeWorker() });
    const before = await engine.layout(roots, "layered");
    const after = await engine.layout(nested, "layered");
    expect(engine.stats).toEqual({ computed: 2, cacheHits: 0 });
    expect(after.hash).not.toBe(before.hash);
    const fresh = await new LayoutEngine({ createWorker: fakeWorker() }).layout(nested, "layered");
    expect(positionBytes(after.positions)).toBe(positionBytes(fresh.positions));
  });

  it("evicts the least recently used layout beyond its cache size", async () => {
    const engine = new LayoutEngine({ createWorker: fakeWorker(), cacheSize: 1 });
    await engine.layout(model, "layered");
    await engine.layout({ ...model, edges: [] }, "layered");
    await engine.layout(model, "layered");
    expect(engine.stats).toEqual({ computed: 3, cacheHits: 0 });
  });

  it("does not cache a failed layout and rejects a malformed reply", async () => {
    let broken = true;
    const tamper = (r: LayoutReply): LayoutReply => (broken && r.ok ? { ...r, xs: r.xs.slice(1) } : r);
    const engine = new LayoutEngine({ createWorker: fakeWorker(tamper) });
    await expect(engine.layout(model, "layered")).rejects.toThrow(/positions for 60 nodes/);
    broken = false;
    await expect(engine.layout(model, "layered")).resolves.toHaveProperty("positions");
    expect(engine.stats).toEqual({ computed: 2, cacheHits: 0 });
  });

  it("rejects replies with missing or non-finite positions, without caching them", async () => {
    const bad: Array<(r: LayoutReply) => unknown> = [
      (r) => ({ id: r.id, ok: true }),
      (r) => (r.ok ? { ...r, xs: Float64Array.from(r.xs, (x, i) => (i === 3 ? NaN : x)) } : r),
      (r) => (r.ok ? { ...r, ys: [...r.ys] } : r),
    ];
    for (const tamper of bad) {
      const engine = new LayoutEngine({ createWorker: fakeWorker(tamper as (r: LayoutReply) => LayoutReply) });
      await expect(engine.layout(model, "layered")).rejects.toThrow(/malformed|non-finite/);
      await expect(engine.layout(model, "layered")).rejects.toThrow(/malformed|non-finite/);
      expect(engine.stats).toEqual({ computed: 2, cacheHits: 0 });
    }
  });

  it("replaces the worker when a reply matches no request", async () => {
    let started = 0;
    const make = fakeWorker((r) => ({ ...r, id: r.id + 100 }));
    const engine = new LayoutEngine({ createWorker: () => { started++; return make(); } });
    await expect(engine.layout(model, "layered")).rejects.toThrow(/matches no request/);
    await expect(engine.layout({ ...model, edges: [] }, "layered")).rejects.toThrow(/matches no request/);
    expect(started).toBe(2);
  });

  it("rejects waiting layouts when the worker fails, then starts a new worker", async () => {
    let started = 0;
    let fail: (error: Error) => void = () => undefined;
    const silent = (): LayoutWorker => {
      started++;
      return { postMessage: () => undefined, onReply: () => undefined, onFailure: (l) => { fail = l; }, terminate: () => undefined };
    };
    const engine = new LayoutEngine({ createWorker: silent });
    const waiting = engine.layout(model, "layered");
    fail(new Error("out of memory"));
    await expect(waiting).rejects.toThrow(/layout worker failed: out of memory/);
    const next = engine.layout({ ...model, edges: [] }, "layered");
    expect(started).toBe(2);
    engine.dispose();
    await expect(next).rejects.toThrow(/disposed/);
  });

  it("rejects an inconsistent model instead of throwing synchronously", async () => {
    const engine = new LayoutEngine({ createWorker: fakeWorker() });
    await expect(engine.layout({ ...model, nodes: [{ id: "x", label: "x", group: "nowhere" }] }, "layered")).rejects.toThrow(/unknown group/);
  });
});
