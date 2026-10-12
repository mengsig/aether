// A hand-written client for the Aether daemon protocol: NDJSON over a Unix
// socket or TCP. The types and command builders beside this file, in
// `protocol.ts`, are generated from the Rust types; this file owns the
// connection itself - handshake, request/reply multiplexing over one
// socket, subscriptions, and the keepalive a tokened TCP transport needs.

import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";

import {
  PROTOCOL_VERSION,
  hello,
  ping,
  subscribe as buildSubscribe,
  unsubscribe as buildUnsubscribe,
} from "./protocol.ts";
import type {
  AgentId,
  Command,
  Delta,
  ErrorCode,
  ErrorReply,
  Event,
  InvocationId,
  Reply,
  SubscribeCommand,
  Welcome,
} from "./protocol.ts";

export const MAX_QUEUED_SUBSCRIPTION_FRAMES = 1024;

/** Terminal overflow. Resume events after the last sequence consumed; deltas are not replayable. */
export class SubscriptionLaggedError extends Error {
  readonly capacity = MAX_QUEUED_SUBSCRIPTION_FRAMES;

  constructor() {
    super(`subscription exceeded ${MAX_QUEUED_SUBSCRIPTION_FRAMES} queued frames; resubscribe after the last event consumed`);
    this.name = "SubscriptionLaggedError";
  }
}

const CLIENT_NAME = "aether-typescript";
const CLIENT_VERSION = "1.0.2";

// Longest accepted line, matching the daemon's own cap (MAX_LINE in
// crates/aether-daemon/src/connection.rs and aether-client/src/transport.rs).
const MAX_LINE = 64 * 1024 * 1024;

// No caller of `request` waits forever on a daemon that has stopped
// answering; see `request`'s doc comment.
const DEFAULT_REQUEST_TIMEOUT_MS = 60_000;

/** The daemon refused a request. `code` is the protocol's `ErrorCode`. */
export class AetherError extends Error {
  readonly code: ErrorCode;

  constructor(code: ErrorCode, message: string) {
    super(`${code}: ${message}`);
    this.name = "AetherError";
    this.code = code;
  }
}

/** One frame pushed to a live subscription. */
export type SubscriptionFrame =
  | { kind: "event"; event: Event }
  | { kind: "delta"; agent: AgentId; invocation: InvocationId; delta: Delta };

export interface ConnectOptions {
  clientName?: string;
  clientVersion?: string;
}

/** Where the daemon listens, resolved the way `aether_protocol::paths::socket_path` resolves it. */
export function defaultSocketPath(): string {
  const envSocket = process.env.AETHER_SOCKET;
  if (envSocket) return envSocket;
  const runtimeDir = process.env.XDG_RUNTIME_DIR;
  if (runtimeDir) return path.join(runtimeDir, "aether", "aetherd.sock");
  return path.join(defaultDataDir(), "aetherd.sock");
}

function defaultDataDir(): string {
  const envData = process.env.AETHER_DATA_DIR;
  if (envData) return envData;
  return path.join(platformDataDir(), "aether");
}

// Mirrors the `dirs` crate's data_dir(): XDG on Linux, Application Support
// on macOS, roaming AppData on Windows.
function platformDataDir(): string {
  if (process.platform === "darwin") {
    return path.join(os.homedir(), "Library", "Application Support");
  }
  if (process.platform === "win32") {
    return process.env.APPDATA ?? path.join(os.homedir(), "AppData", "Roaming");
  }
  return process.env.XDG_DATA_HOME || path.join(os.homedir(), ".local", "share");
}

// A minimal async FIFO: push() from the reader, next() from the consumer.
// Keeps a bounded prefix, then reports overflow instead of losing frames silently.
class AsyncQueue<T> {
  private readonly items: T[] = [];
  private readonly waiters: Array<(result: IteratorResult<T>) => void> = [];
  private failure?: Error;
  private ended = false;

  push(item: T): boolean {
    if (this.ended) return false;
    const waiter = this.waiters.shift();
    if (waiter) {
      waiter({ value: item, done: false });
    } else if (this.items.length < MAX_QUEUED_SUBSCRIPTION_FRAMES) {
      this.items.push(item);
    } else {
      this.failure = new SubscriptionLaggedError();
      this.end();
      return false;
    }
    return true;
  }

  end(): void {
    if (this.ended) return;
    this.ended = true;
    while (this.waiters.length > 0) {
      this.waiters.shift()!({ value: undefined, done: true });
    }
  }

  next(): Promise<IteratorResult<T>> {
    if (this.items.length > 0) {
      return Promise.resolve({ value: this.items.shift()!, done: false });
    }
    if (this.failure) return Promise.reject(this.failure);
    if (this.ended) {
      return Promise.resolve({ value: undefined, done: true });
    }
    return new Promise((resolve) => this.waiters.push(resolve));
  }
}

/** A live subscription: iterate it for the event/delta frames it carries. */
export class Subscription implements AsyncIterableIterator<SubscriptionFrame> {
  readonly id: number;
  readonly lastSeq: number;
  private readonly client: AetherClient;
  private readonly queue: AsyncQueue<SubscriptionFrame>;
  private released = false;

  constructor(client: AetherClient, id: number, lastSeq: number, queue: AsyncQueue<SubscriptionFrame>) {
    this.client = client;
    this.id = id;
    this.lastSeq = lastSeq;
    this.queue = queue;
  }

  next(): Promise<IteratorResult<SubscriptionFrame>> {
    return this.queue.next();
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<SubscriptionFrame> {
    return this;
  }

  /** Stop delivering frames for this subscription and tell the daemon to drop it. */
  async return(): Promise<IteratorResult<SubscriptionFrame>> {
    await this.unsubscribe();
    return { value: undefined, done: true };
  }

  async unsubscribe(): Promise<void> {
    if (this.released) return;
    this.client.dropSubscription(this.id);
    await this.client.request(buildUnsubscribe({ id: this.id }));
    this.released = true;
  }
}

interface PendingRequest {
  resolve(reply: Reply): void;
  reject(err: Error): void;
}

// The wire shape of a reply frame's `result`: `{"Ok": <Reply>}` or
// `{"Err": <ErrorReply>}`. protocol.ts types this loosely (it is a Rust
// `Result`, not a tagged struct), so it is narrowed here instead.
interface RawResult {
  Ok?: Reply;
  Err?: ErrorReply;
}

type RawFrame =
  | { frame: "reply"; id: number; result: RawResult }
  | { frame: "event"; sub: number; event: Event }
  | { frame: "delta"; sub: number; agent: AgentId; invocation: InvocationId; delta: Delta };

function connectSocket(factory: () => net.Socket): Promise<net.Socket> {
  return new Promise((resolve, reject) => {
    const socket = factory();
    const onError = (err: Error) => {
      socket.removeListener("connect", onConnect);
      reject(err);
    };
    const onConnect = () => {
      socket.removeListener("error", onError);
      resolve(socket);
    };
    socket.once("error", onError);
    socket.once("connect", onConnect);
  });
}

/**
 * One connection to the daemon. Every request carries its own id, so any
 * number of callers can share a client; a background reader routes replies
 * back to the waiting caller and pushes subscription frames to their queue.
 */
export class AetherClient {
  private readonly socket: net.Socket;
  private buffer: Buffer = Buffer.alloc(0);
  private nextReq = 1;
  private closed = false;
  private readonly pending = new Map<number, PendingRequest>();
  private readonly subscriptions = new Map<number, AsyncQueue<SubscriptionFrame>>();
  private readonly subscriptionReceivers = new Map<number, AsyncQueue<SubscriptionFrame>>();
  private keepaliveTimer: NodeJS.Timeout | undefined;
  /** The daemon's last notice on request id 0; see `handleReply`. */
  private notice: AetherError | undefined;
  private _welcome!: Welcome;

  private constructor(socket: net.Socket) {
    this.socket = socket;
    socket.on("data", (chunk: Buffer) => this.onData(chunk));
    socket.on("error", (err: Error) => this.fail(err));
    socket.on("close", () =>
      this.fail(this.notice ?? new Error("the daemon closed the connection")),
    );
  }

  /** What the daemon said before closing, when it said anything. */
  get disconnectReason(): AetherError | undefined {
    return this.notice;
  }

  /** The handshake reply. Set once `connectUnix`/`connectTcp` resolve. */
  get welcome(): Welcome {
    return this._welcome;
  }

  get isConnected(): boolean {
    return !this.closed;
  }

  /** Connect over the Unix socket, resolved with `defaultSocketPath()` when omitted. */
  static async connectUnix(path?: string, options?: ConnectOptions): Promise<AetherClient> {
    const socket = await connectSocket(() => net.createConnection({ path: path ?? defaultSocketPath() }));
    return AetherClient.handshake(socket, options);
  }

  /** Connect over TCP. The transport authorizes on `token` alone. */
  static async connectTcp(host: string, port: number, token: string, options?: ConnectOptions): Promise<AetherClient> {
    const socket = await connectSocket(() => net.createConnection({ host, port }));
    socket.setNoDelay(true);
    return AetherClient.handshake(socket, options, token);
  }

  private static async handshake(socket: net.Socket, options?: ConnectOptions, token?: string): Promise<AetherClient> {
    const client = new AetherClient(socket);
    try {
      const reply = await client.request(
        hello({
          client_name: options?.clientName ?? CLIENT_NAME,
          client_version: options?.clientVersion ?? CLIENT_VERSION,
          protocol_version: PROTOCOL_VERSION,
          features: ["deltas", "artifact_bytes"],
          ...(token !== undefined ? { token } : {}),
        }),
      );
      if (reply.reply !== "welcome") {
        throw new Error(`expected a "welcome" reply, got "${reply.reply}"`);
      }
      client._welcome = reply.data;
      client.startKeepalive();
      return client;
    } catch (err) {
      client.close();
      throw err;
    }
  }

  /**
   * Send a command and resolve with its `Ok` reply, or reject with an
   * `AetherError`. Rejects after `timeoutMs` (default 60s, matching the
   * Python client) if no reply arrives; pass 0 to wait forever.
   */
  request(command: Command, timeoutMs: number = DEFAULT_REQUEST_TIMEOUT_MS): Promise<Reply> {
    if (command.cmd === "subscribe") {
      return Promise.reject(new Error("use AetherClient.subscribe to receive subscription frames"));
    }
    return this.requestInner(command, timeoutMs);
  }

  private requestInner(command: Command, timeoutMs: number = DEFAULT_REQUEST_TIMEOUT_MS): Promise<Reply> {
    if (this.closed) {
      return Promise.reject(new Error("connection is closed"));
    }
    const req = this.nextReq++;
    let line: string;
    try {
      const encoded = JSON.stringify({ req, ...command });
      if (encoded === undefined) throw new TypeError("command could not be serialized");
      line = encoded + "\n";
    } catch (err) {
      return Promise.reject(err);
    }
    return new Promise<Reply>((resolve, reject) => {
      const timer =
        timeoutMs > 0
          ? setTimeout(() => {
              if (this.pending.delete(req)) {
                reject(new Error(`no reply to request ${req} within ${timeoutMs}ms`));
              }
            }, timeoutMs)
          : undefined;
      timer?.unref();
      this.pending.set(req, {
        resolve: (reply) => {
          if (timer) clearTimeout(timer);
          resolve(reply);
        },
        reject: (err) => {
          if (timer) clearTimeout(timer);
          reject(err);
        },
      });
      this.socket.write(line, (err) => {
        if (err) {
          if (timer) clearTimeout(timer);
          this.pending.delete(req);
          reject(err);
        }
      });
    });
  }

  /** Open a subscription. Frames for it arrive on the returned handle. */
  async subscribe(args: Omit<SubscribeCommand, "cmd">): Promise<Subscription> {
    const reply = await this.requestInner(buildSubscribe(args));
    if (reply.reply !== "subscribed") {
      throw new Error(`expected a "subscribed" reply, got "${reply.reply}"`);
    }
    const data = reply.data as { id: number; last_seq: number };
    const queue = this.subscriptionReceivers.get(data.id);
    this.subscriptionReceivers.delete(data.id);
    if (!queue) {
      throw new Error(`internal: no queue registered for subscription ${data.id}`);
    }
    return new Subscription(this, data.id, data.last_seq, queue);
  }

  /** @internal used by `Subscription.unsubscribe`. */
  dropSubscription(id: number): void {
    this.subscriptions.get(id)?.end();
    this.subscriptions.delete(id);
    this.subscriptionReceivers.delete(id);
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.clearKeepalive();
    this.socket.destroy();
    this.settleAll(new Error("connection closed"));
  }

  private fail(err: Error): void {
    if (this.closed) return;
    this.closed = true;
    this.clearKeepalive();
    this.socket.destroy();
    this.settleAll(err);
  }

  private settleAll(err: Error): void {
    for (const waiter of this.pending.values()) waiter.reject(err);
    this.pending.clear();
    for (const queue of this.subscriptions.values()) queue.end();
    this.subscriptions.clear();
    this.subscriptionReceivers.clear();
  }

  // Hold a quiet connection open under the daemon's idle timeout. Started
  // only when the welcome reports one, matching aether-client's keep_alive.
  private startKeepalive(): void {
    const secs = this._welcome.idle_timeout_secs;
    if (secs === undefined || secs === null || secs < 3) return;
    const timer = setInterval(() => {
      if (this.closed) return;
      // A failed ping means the connection already died; that surfaces
      // through the socket's own close/error handlers, not here.
      this.request(ping()).catch(() => {});
    }, Math.floor(secs / 3) * 1000);
    timer.unref();
    this.keepaliveTimer = timer;
  }

  private clearKeepalive(): void {
    if (this.keepaliveTimer === undefined) return;
    clearInterval(this.keepaliveTimer);
    this.keepaliveTimer = undefined;
  }

  private onData(chunk: Buffer): void {
    this.buffer = this.buffer.length > 0 ? Buffer.concat([this.buffer, chunk]) : chunk;
    let newline: number;
    while ((newline = this.buffer.indexOf(0x0a)) >= 0) {
      const line = this.buffer.subarray(0, newline).toString("utf8");
      this.buffer = this.buffer.subarray(newline + 1);
      this.handleLine(line);
      if (this.closed) return;
    }
    if (this.buffer.length > MAX_LINE) {
      this.fail(new Error(`line exceeds the ${MAX_LINE} byte limit with no terminator`));
    }
  }

  private handleLine(line: string): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch (err) {
      this.fail(new Error(`could not parse frame as JSON: ${(err as Error).message}`));
      return;
    }
    // A hostile or buggy peer on this transport is not bound to send an
    // object at all; dereferencing `.frame` on anything else throws, and an
    // uncaught throw here takes the embedding process down with it.
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      this.fail(new Error(`server frame is not an object: ${line}`));
      return;
    }
    const frame = parsed as RawFrame;
    switch (frame.frame) {
      case "reply":
        this.handleReply(frame.id, frame.result);
        return;
      case "event":
        this.deliver(frame.sub, { kind: "event", event: frame.event });
        return;
      case "delta":
        this.deliver(frame.sub, {
          kind: "delta",
          agent: frame.agent,
          invocation: frame.invocation,
          delta: frame.delta,
        });
        return;
      default:
        this.fail(new Error(`unknown server frame: ${line}`));
    }
  }

  private deliver(id: number, frame: SubscriptionFrame): void {
    const queue = this.subscriptions.get(id);
    if (queue && !queue.push(frame)) {
      this.subscriptions.delete(id);
      this.releaseSubscription(id);
    }
  }

  private releaseSubscription(id: number): void {
    void this.request(buildUnsubscribe({ id })).catch((error: Error) => {
      console.warn(`could not release subscription ${id}: ${error.message}`);
    });
  }

  private handleReply(id: number, result: RawResult | undefined): void {
    // Request ids start at 1, so 0 is the daemon speaking on its own account
    // rather than answering anything - the reason a connection is about to
    // close. Kept, so the close that follows says why.
    if (id === 0) {
      if (result?.Err !== undefined) {
        this.notice = new AetherError(result.Err.code, result.Err.message);
      }
      return;
    }
    // A malformed reply - no `result` at all, or one that is not an object -
    // must fail the connection, not throw out of dereferencing `.Ok`/`.Err`.
    if (result === undefined || result === null || typeof result !== "object") {
      this.fail(new Error(`the daemon sent a reply with no result: ${JSON.stringify(result)}`));
      return;
    }
    if (result.Ok !== undefined) {
      const reply = result.Ok;
      // Register the subscription's queue before resolving the caller's
      // promise, so an event/delta frame that arrives before the caller
      // starts iterating is still buffered, not dropped.
      if (reply.reply === "subscribed") {
        const data = reply.data as { id: number };
        if (!this.pending.has(id)) {
          this.releaseSubscription(data.id);
          return;
        }
        const queue = new AsyncQueue<SubscriptionFrame>();
        this.subscriptions.set(data.id, queue);
        this.subscriptionReceivers.set(data.id, queue);
      }
      this.pending.get(id)?.resolve(reply);
      this.pending.delete(id);
      return;
    }
    if (result.Err !== undefined) {
      this.pending.get(id)?.reject(new AetherError(result.Err.code, result.Err.message));
      this.pending.delete(id);
      return;
    }
    this.fail(new Error(`malformed reply result: ${JSON.stringify(result)}`));
  }
}
