// Runs the real layout.worker.ts inside a Node worker thread by giving it the two Worker globals it uses.

import { parentPort } from "node:worker_threads";

const port = parentPort;
if (port === null) throw new Error("layout-worker-host runs only as a worker thread");
Object.assign(globalThis, {
  addEventListener: (type: string, listener: (event: { data: unknown }) => void) => {
    if (type !== "message") throw new Error(`the layout worker listens only for messages, not ${type}`);
    port.on("message", (data: unknown) => listener({ data }));
  },
  postMessage: (message: unknown, transfer: ArrayBuffer[]) => port.postMessage(message, transfer),
});
await import("../src/layout.worker.ts");
