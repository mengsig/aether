// Keep Playwright's CDP session from forcing focus on Electron's non-default context.
import assert from "node:assert/strict";
import { once } from "node:events";
import { WebSocket, WebSocketServer, type RawData } from "ws";

function withoutFocusEmulation(data: RawData): string {
  const text = data.toString();
  const message: unknown = JSON.parse(text);
  assert.ok(message !== null && typeof message === "object", "a CDP message is an object");
  const command = message as { method?: string; params?: { enabled?: boolean } };
  if (command.method !== "Emulation.setFocusEmulationEnabled") return text;
  assert.ok(command.params !== undefined && typeof command.params.enabled === "boolean", "focus emulation has a boolean setting");
  command.params.enabled = false;
  return JSON.stringify(message);
}

/** A loopback proxy that changes only Playwright's focus emulation command. */
export async function cdpProxy(port: number): Promise<{ endpoint: string; close: () => Promise<void> }> {
  assert.ok(Number.isInteger(port) && port > 0 && port <= 65535, "the DevTools port is valid");
  const response = await fetch(`http://127.0.0.1:${port}/json/version`);
  assert.ok(response.ok, "DevTools exposes its browser endpoint");
  const version = await response.json() as { webSocketDebuggerUrl?: unknown };
  assert.ok(typeof version.webSocketDebuggerUrl === "string" && version.webSocketDebuggerUrl.startsWith(`ws://127.0.0.1:${port}/`), "the endpoint is local");
  const server = new WebSocketServer({ host: "127.0.0.1", port: 0 });
  server.on("connection", (client) => {
    const upstream = new WebSocket(version.webSocketDebuggerUrl as string);
    const pending: string[] = [];
    client.on("message", (data) => {
      const message = withoutFocusEmulation(data);
      if (upstream.readyState === WebSocket.OPEN) upstream.send(message);
      else pending.push(message);
    });
    upstream.on("open", () => { for (const message of pending) upstream.send(message); });
    upstream.on("message", (data) => client.send(data));
    client.on("close", () => upstream.close());
    upstream.on("close", () => client.close());
    client.on("error", (error) => { upstream.terminate(); throw error; });
    upstream.on("error", (error) => { client.terminate(); throw error; });
  });
  await once(server, "listening");
  const address = server.address();
  assert.ok(address !== null && typeof address !== "string", "the proxy has a local port");
  return { endpoint: `ws://127.0.0.1:${address.port}`, close: () => new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())) };
}
