// The page-wide layout engine on a browser Worker running layout.worker.ts.

import type { GraphModel, Layout } from "@aether-ui/protocol";
import { LayoutEngine, type LayoutWorker } from "./engine.ts";

/** Adapts a browser Worker running layout.worker.ts. */
export function browserLayoutWorker(worker: Worker): LayoutWorker {
  return {
    postMessage: (request) => worker.postMessage(request),
    onReply: (listener) => worker.addEventListener("message", (event: MessageEvent<unknown>) => listener(event.data)),
    onFailure: (listener) => {
      worker.addEventListener("error", (event) => listener(new Error(event.message || "worker error")));
      worker.addEventListener("messageerror", () => listener(new Error("a layout reply could not be read")));
    },
    terminate: () => worker.terminate(),
  };
}

let shared: LayoutEngine | undefined;

/** The page-wide engine behind `layeredLayout` and `forceLayout`, created on first use. */
export function sharedLayoutEngine(): LayoutEngine {
  shared ??= new LayoutEngine({
    createWorker: () => browserLayoutWorker(new Worker(new URL("./layout.worker.ts", import.meta.url), { type: "module" })),
  });
  return shared;
}

/** Layered DAG layout in the worker, cached by the structure of the visible graph. */
export function layeredLayout(model: GraphModel): Promise<Layout> {
  return sharedLayoutEngine().layout(model, "layered");
}

/** Force layout (d3-force) in the worker for the knowledge graph, cached the same way. */
export function forceLayout(model: GraphModel): Promise<Layout> {
  return sharedLayoutEngine().layout(model, "force");
}
