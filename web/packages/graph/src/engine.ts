// Runs layouts in a worker and caches them by structure, so a status or label change returns the
// cached Layout without a new computation. The cache is keyed by the full structure key, not its hash.

import type { GraphModel, Layout } from "@aether-ui/protocol";
import { at, invariant, must } from "./invariant.ts";
import { visibleGraph, type VisibleGraph } from "./model.ts";
import type { Algorithm, LayoutReply, LayoutRequest } from "./worker-protocol.ts";

/** The part of a Worker the engine uses; tests pass a Node worker_threads adapter. */
export interface LayoutWorker {
  postMessage(request: LayoutRequest): void;
  onReply(listener: (reply: LayoutReply) => void): void;
  onFailure(listener: (error: Error) => void): void;
  terminate(): void;
}

export interface LayoutEngineOptions { createWorker: () => LayoutWorker; cacheSize?: number }
export interface LayoutStats { computed: number; cacheHits: number }

interface Pending { graph: VisibleGraph; resolve: (layout: Layout) => void; reject: (error: Error) => void }

export class LayoutEngine {
  private readonly createWorker: () => LayoutWorker;
  private readonly cacheSize: number;
  private readonly cache = new Map<string, Promise<Layout>>();
  private readonly pending = new Map<number, Pending>();
  private worker: LayoutWorker | undefined;
  private nextId = 1;
  private counts: LayoutStats = { computed: 0, cacheHits: 0 };

  constructor(options: LayoutEngineOptions) {
    this.createWorker = options.createWorker;
    this.cacheSize = options.cacheSize ?? 16;
    invariant(Number.isInteger(this.cacheSize) && this.cacheSize > 0, "the layout cache holds at least one layout");
  }

  /** Computations sent to the worker and requests answered from the cache. */
  get stats(): LayoutStats {
    return { ...this.counts };
  }

  /** Async so a GraphModelError rejects; it runs synchronously up to the cache decision. */
  async layout(model: GraphModel, algorithm: Algorithm): Promise<Layout> {
    const graph = visibleGraph(model);
    const cacheKey = `${algorithm}\n${graph.key}`;
    const cached = this.cache.get(cacheKey);
    if (cached !== undefined) {
      this.counts.cacheHits++;
      this.cache.delete(cacheKey);
      this.cache.set(cacheKey, cached);
      return cached;
    }
    const result = this.compute(graph, algorithm);
    this.cache.set(cacheKey, result);
    const oldest = this.cache.keys().next();
    if (this.cache.size > this.cacheSize && oldest.done !== true) this.cache.delete(oldest.value);
    // A failed layout is not cached, so the next request computes again.
    result.catch(() => { if (this.cache.get(cacheKey) === result) this.cache.delete(cacheKey); });
    return result;
  }

  dispose(): void {
    this.failAll(new Error("the layout engine was disposed"));
    this.cache.clear();
  }

  private compute(graph: VisibleGraph, algorithm: Algorithm): Promise<Layout> {
    this.counts.computed++;
    const index = new Map(graph.nodes.map((node, i) => [node.id, i]));
    const edges: number[] = [];
    for (const edge of graph.edges) edges.push(must(index, edge.from), must(index, edge.to));
    const request: LayoutRequest = {
      id: this.nextId++, algorithm, edges,
      nodes: graph.nodes.map((node) => node.id),
      clusters: graph.nodes.map((node) => node.group ?? ""),
    };
    return new Promise<Layout>((resolve, reject) => {
      this.pending.set(request.id, { graph, resolve, reject });
      this.ensureWorker().postMessage(request);
    });
  }

  private ensureWorker(): LayoutWorker {
    if (this.worker !== undefined) return this.worker;
    const worker = this.createWorker();
    worker.onReply((reply) => this.settle(reply));
    worker.onFailure((error) => {
      if (this.worker === worker) this.failAll(new Error(`the layout worker failed: ${error.message}`, { cause: error }));
    });
    this.worker = worker;
    return worker;
  }

  private settle(reply: LayoutReply): void {
    const pending = this.pending.get(reply.id);
    invariant(pending !== undefined, `layout reply ${reply.id} matches no request`);
    this.pending.delete(reply.id);
    if (!reply.ok) return pending.reject(new Error(`layout failed: ${reply.error}`));
    const { nodes, hash } = pending.graph;
    if (reply.xs.length !== nodes.length || reply.ys.length !== nodes.length) {
      return pending.reject(new Error(`layout reply has ${reply.xs.length} positions for ${nodes.length} nodes`));
    }
    const positions = new Map<string, { x: number; y: number }>();
    nodes.forEach((node, i) => positions.set(node.id, { x: at(reply.xs, i), y: at(reply.ys, i) }));
    pending.resolve({ hash, positions });
  }

  /** Rejects every waiting request and drops the worker; the next request starts a new one. */
  private failAll(error: Error): void {
    for (const pending of this.pending.values()) pending.reject(error);
    this.pending.clear();
    this.worker?.terminate();
    this.worker = undefined;
  }
}
