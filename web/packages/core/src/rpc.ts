// Request and reply matching over one open transport link: every request
// gets a `req`, every reply settles exactly one, and a close fails the rest.

import { DaemonError } from "./errors.ts";
import { invariant, object, seq, string, type Json } from "./guards.ts";

export interface Link { send(frame: unknown): void; close(): void }

/** Any client command: a generated command type, or a literal checked for its `cmd` at runtime. */
export type Command = { readonly cmd: string } | Json;

interface Pending { resolve: (reply: Json) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }

export const REQUEST_TIMEOUT_MS = 20_000;
const MAX_PENDING = 128;

export class Rpc {
  private readonly pending = new Map<number, Pending>();
  private nextReq = 1;
  private closed: Error | null = null;
  private readonly link: Link;
  private readonly timeoutMs: number;

  constructor(link: Link, timeoutMs = REQUEST_TIMEOUT_MS) {
    invariant(timeoutMs > 0, "request timeout must be positive");
    this.link = link;
    this.timeoutMs = timeoutMs;
  }

  /** Sends one command; resolves with the `Ok` reply object, rejects with a DaemonError. */
  request(command: Command): Promise<Json> {
    const cmd = string((command as Json).cmd, "command.cmd");
    if (this.closed) return Promise.reject(this.closed);
    if (this.pending.size >= MAX_PENDING) return Promise.reject(new Error("Too many requests in flight"));
    const req = this.nextReq++;
    return new Promise<Json>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(req);
        reject(new DaemonError("timeout", `${cmd} got no answer in ${Math.round(this.timeoutMs / 1000)} s`));
      }, this.timeoutMs);
      this.pending.set(req, { resolve, reject, timer });
      try {
        this.link.send({ ...command, req });
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(req);
        reject(new Error(`Could not send ${cmd}`, { cause: error }));
      }
    });
  }

  /**
   * Settles a reply frame. Returns the id-0 diagnostic error, if the frame was
   * one, so the caller can act on the close the daemon announces.
   */
  settle(frame: Json): DaemonError | null {
    const id = seq(frame.id, "reply.id");
    const result = object(frame.result, "reply.result");
    invariant(Object.hasOwn(result, "Ok") !== Object.hasOwn(result, "Err"), "a reply holds exactly one of Ok and Err");
    const error = Object.hasOwn(result, "Err") ? DaemonError.fromReply(result.Err) : null;
    if (id === 0) return error ?? new DaemonError("internal", "unaddressed reply without an error");
    const entry = this.pending.get(id);
    if (!entry) return null;
    this.pending.delete(id);
    clearTimeout(entry.timer);
    if (error) entry.reject(error);
    else entry.resolve(object(result.Ok, "reply.result.Ok"));
    return null;
  }

  /** Fails every pending and future request; a request already sent may or may not have acted. */
  fail(error: Error): void {
    this.closed ??= error;
    for (const entry of this.pending.values()) {
      clearTimeout(entry.timer);
      entry.reject(new DaemonError("disconnected", `The link closed before the answer: ${error.message}`, {}, { cause: error }));
    }
    this.pending.clear();
    invariant(this.pending.size === 0, "a failed link has nothing pending");
  }

  get inFlight(): number {
    return this.pending.size;
  }
}
