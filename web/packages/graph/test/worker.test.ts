import { Worker } from "node:worker_threads";
import { describe, expect, it } from "vitest";
import { LayoutEngine, layoutInput, type LayoutWorker } from "../src/engine.ts";
import { layered } from "../src/layered.ts";
import { visibleGraph } from "../src/model.ts";
import type { LayoutReply } from "../src/worker-protocol.ts";
import { positionBytes, seededGraph } from "./graphs.ts";

function threadWorker(): LayoutWorker {
  const worker = new Worker(new URL("./layout-worker-host.ts", import.meta.url));
  return {
    postMessage: (request) => worker.postMessage(request),
    onReply: (listener) => worker.on("message", (reply: LayoutReply) => listener(reply)),
    onFailure: (listener) => worker.on("error", listener),
    terminate: () => void worker.terminate(),
  };
}

describe("layeredLayout in a worker thread", () => {
  it("lays out 2,000 nodes and 4,000 edges in under 1.5 s, matching the in-process bytes", async () => {
    const model = seededGraph(2000, 4000);
    const engine = new LayoutEngine({ createWorker: threadWorker });
    // One bounded run: the time includes starting the worker and loading its modules.
    const started = performance.now();
    const layout = await engine.layout(model, "layered");
    const elapsed = performance.now() - started;
    engine.dispose();
    console.log(`layered layout, 2000 nodes / 4000 edges, worker thread: ${elapsed.toFixed(0)} ms`);
    expect(elapsed).toBeLessThan(1500);
    expect(layout.positions.size).toBe(visibleGraph(model).nodes.length);

    const graph = visibleGraph(model);
    const local = layered(layoutInput(graph));
    const expected = new Map(graph.nodes.map((n, i) => [n.id, { x: local.xs[i] ?? NaN, y: local.ys[i] ?? NaN }]));
    expect(positionBytes(layout.positions)).toBe(positionBytes(expected));
  });
});
