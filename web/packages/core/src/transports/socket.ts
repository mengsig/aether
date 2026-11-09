// One WebSocket with JSON text frames, shared by the ws-aether and ws-archon
// transports. Tests inject a fake through SocketFactory.

import { invariant, object, type Json } from "../guards.ts";

export interface SocketLike {
  readonly readyState: number;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: { code: number; reason: string }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  send(data: string): void;
  close(code?: number, reason?: string): void;
}

export type SocketFactory = (url: string, protocol: string) => SocketLike;

export const MAX_FRAME_BYTES = 64 * 1024 * 1024;
const OPEN = 1;

export const browserSocket: SocketFactory = (url, protocol) => new WebSocket(url, protocol) as unknown as SocketLike;

export function parseFrame(raw: unknown): Json {
  if (typeof raw !== "string" || raw.length > MAX_FRAME_BYTES) throw new TypeError("Invalid or oversized WebSocket text frame");
  return object(JSON.parse(raw), "frame");
}

export function sendFrame(socket: SocketLike, frame: unknown): void {
  invariant(socket.readyState === OPEN, "the WebSocket is open");
  const text = JSON.stringify(frame);
  if (text.length > MAX_FRAME_BYTES) throw new RangeError("Frame exceeds the 64 MiB limit");
  socket.send(text);
}

export function checkSocketUrl(target: string, path: string): string {
  const url = new URL(target);
  if (url.protocol !== "ws:" && url.protocol !== "wss:") throw new TypeError(`Not a WebSocket URL: ${target}`);
  if (url.pathname !== path) throw new TypeError(`The socket must be at ${path}, not ${url.pathname}`);
  return url.href;
}

/**
 * Opens a socket and resolves once it is open. Messages go to onText; the
 * close after open goes to onClose, a failure before open rejects instead.
 */
export function openSocket(factory: SocketFactory, url: string, protocol: string, handlers: {
  onText(raw: unknown): void;
  onClose(code: number, reason: string): void;
}, signal?: AbortSignal): Promise<SocketLike> {
  return new Promise((resolve, reject) => {
    const socket = factory(url, protocol);
    let opened = false;
    let cancelled = false;
    const abort = () => {
      cancelled = true;
      socket.onopen = null;
      socket.onmessage = null;
      socket.onclose = null;
      socket.onerror = null;
      socket.close();
      reject(new DOMException("Connection attempt cancelled", "AbortError"));
    };
    if (signal?.aborted) return abort();
    signal?.addEventListener("abort", abort, { once: true });
    socket.onopen = () => {
      if (cancelled) return;
      opened = true;
      signal?.removeEventListener("abort", abort);
      resolve(socket);
    };
    socket.onmessage = (event) => handlers.onText(event.data);
    // Errors always end in a close event, which carries the handling.
    socket.onerror = () => {};
    socket.onclose = (event) => {
      signal?.removeEventListener("abort", abort);
      if (opened) handlers.onClose(event.code, event.reason);
      else reject(new Error(`WebSocket ${url} closed before it opened (${event.code})`));
    };
  });
}
