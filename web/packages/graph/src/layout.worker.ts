// Layout worker entry: one reply per request, positions transferred rather than copied.

import { runRequest, type LayoutRequest, type LayoutReply } from "./worker-protocol.ts";

interface WorkerScope {
  addEventListener(type: "message", listener: (event: { data: LayoutRequest }) => void): void;
  postMessage(message: LayoutReply, transfer: ArrayBuffer[]): void;
}

const scope = globalThis as unknown as WorkerScope;
scope.addEventListener("message", (event) => {
  const reply = runRequest(event.data);
  scope.postMessage(reply, reply.ok ? [reply.xs.buffer as ArrayBuffer, reply.ys.buffer as ArrayBuffer] : []);
});
