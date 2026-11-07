// The layout worker's work on the calling thread, for tests and measurements outside a browser:
// the same requests and replies as layout.worker.ts, answered on a microtask.

import type { LayoutWorker } from "./engine.ts";
import { invariant } from "./invariant.ts";
import { runRequest, type LayoutReply, type LayoutRequest } from "./worker-protocol.ts";

export function inlineLayoutWorker(): LayoutWorker {
  let listener: ((reply: unknown) => void) | undefined;
  let open = true;
  return {
    postMessage: (request: LayoutRequest) => {
      invariant(open, "a terminated inline worker takes no requests");
      invariant(listener !== undefined, "the engine listens before it asks");
      const reply: LayoutReply = runRequest(request);
      queueMicrotask(() => { if (open) listener?.(reply); });
    },
    onReply: (next) => { listener = next; },
    onFailure: () => undefined,
    terminate: () => { open = false; },
  };
}
