// One Unix socket to a local daemon, speaking newline-delimited JSON frames as the CLIs do
// (clients/typescript/src/client.ts). Replies settle requests by id; other frames go to onFrame.

import net from "node:net";
import { MAX_DAEMON_LINE_BYTES } from "../security/limits.ts";

export type Frame = Readonly<Record<string, unknown>>;

/** The part of a socket a link uses; tests hand in an in-memory pair. */
export interface LinkSocket {
  write(data: string): boolean;
  end(): void;
  destroy(): void;
  on(event: "data", listener: (chunk: Buffer) => void): this;
  on(event: "close", listener: () => void): this;
  on(event: "error", listener: (error: Error) => void): this;
}

interface Pending {
  resolve(frame: Frame): void;
  reject(error: Error): void;
  timer: NodeJS.Timeout;
}

export const REQUEST_TIMEOUT_MS = 60_000;

/** What became of a daemon's reply: the `Ok` reply or an error naming the daemon's code. */
export function okData(reply: Frame): unknown {
  const result = reply.result as { Ok?: { data?: unknown }; Err?: { code?: unknown; message?: unknown } } | undefined;
  if (result?.Ok !== undefined) return result.Ok.data;
  if (result?.Err !== undefined) throw new Error(`${String(result.Err.code)}: ${String(result.Err.message)}`);
  throw new Error("The daemon's reply carries neither Ok nor Err");
}

export class DaemonLink {
  private readonly socket: LinkSocket;
  private readonly pending = new Map<number, Pending>();
  private readonly chunks: Buffer[] = [];
  private buffered = 0;
  private closed = false;
  onFrame: (frame: Frame) => void = () => {};
  onClose: (reason: string) => void = () => {};

  constructor(socket: LinkSocket) {
    this.socket = socket;
    socket.on("data", (chunk) => this.receive(chunk));
    socket.on("error", (error) => this.fail(`socket error: ${error.message}`));
    socket.on("close", () => this.fail("the daemon closed the connection"));
  }

  static connect(path: string, timeoutMs = 5_000): Promise<DaemonLink> {
    return new Promise((resolve, reject) => {
      const socket = net.createConnection({ path });
      const timer = setTimeout(() => {
        socket.destroy();
        reject(new Error(`No answer from ${path} within ${timeoutMs} ms`));
      }, timeoutMs);
      socket.once("error", (error) => {
        clearTimeout(timer);
        reject(new Error(`Could not connect to ${path}: ${error.message}`, { cause: error }));
      });
      socket.once("connect", () => {
        clearTimeout(timer);
        socket.removeAllListeners("error");
        resolve(new DaemonLink(socket));
      });
    });
  }

  get isOpen(): boolean {
    return !this.closed;
  }

  hasPending(req: number): boolean {
    return this.pending.has(req);
  }

  /** Writes `text`, one JSON frame, as one line. */
  send(text: string): void {
    if (this.closed) throw new Error("The connection to the daemon is closed");
    if (text.includes("\n") || !text.startsWith("{")) throw new Error("a frame is one line of JSON object text");
    this.socket.write(`${text}\n`);
  }

  /** Sends `text`, whose request id is `req`, and resolves with the reply frame for it. */
  request(text: string, req: number, timeoutMs = REQUEST_TIMEOUT_MS): Promise<Frame> {
    if (!Number.isSafeInteger(req) || req < 1) throw new Error(`request ids are positive integers, not ${req}`);
    if (this.pending.has(req)) return Promise.reject(new Error(`Request ${req} is already waiting for its reply`));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.pending.delete(req)) reject(new Error(`No reply to request ${req} within ${timeoutMs} ms`));
      }, timeoutMs);
      this.pending.set(req, { resolve, reject, timer });
      try {
        this.send(text);
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(req);
        reject(error as Error);
      }
    });
  }

  close(): void {
    this.fail("closed by the desktop app");
    this.socket.end();
  }

  private receive(chunk: Buffer): void {
    let start = 0;
    let newline = chunk.indexOf(0x0a);
    while (newline !== -1) {
      this.chunks.push(chunk.subarray(start, newline));
      const line = Buffer.concat(this.chunks).toString("utf8");
      this.chunks.length = 0;
      this.buffered = 0;
      if (line.length > 0) this.deliver(line);
      start = newline + 1;
      newline = chunk.indexOf(0x0a, start);
    }
    this.chunks.push(chunk.subarray(start));
    this.buffered += chunk.length - start;
    if (this.buffered > MAX_DAEMON_LINE_BYTES) {
      this.fail("the daemon sent a frame over the 64 MiB limit");
      this.socket.destroy();
    }
  }

  private deliver(line: string): void {
    let frame: unknown;
    try {
      frame = JSON.parse(line);
    } catch (error) {
      this.fail(`the daemon sent a line that is not JSON: ${(error as Error).message}`);
      this.socket.destroy();
      return;
    }
    if (typeof frame !== "object" || frame === null || Array.isArray(frame)) {
      this.fail("the daemon sent a frame that is not a JSON object");
      this.socket.destroy();
      return;
    }
    const reply = frame as Frame;
    const waiting = reply.frame === "reply" && typeof reply.id === "number" ? this.pending.get(reply.id) : undefined;
    if (waiting === undefined) {
      this.onFrame(reply);
      return;
    }
    this.pending.delete(reply.id as number);
    clearTimeout(waiting.timer);
    waiting.resolve(reply);
  }

  private fail(reason: string): void {
    if (this.closed) return;
    this.closed = true;
    for (const [req, waiting] of this.pending) {
      clearTimeout(waiting.timer);
      waiting.reject(new Error(`Request ${req} failed: ${reason}`));
    }
    this.pending.clear();
    this.onClose(reason);
  }
}
