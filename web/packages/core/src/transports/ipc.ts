// The `ipc` transport over Electron main's bridge (CONTRACTS.md §2.4): requests
// map to bridge.request, subscriptions to bridge.subscribe; both answer frames.

import type { DesktopBridge, Transport } from "@aether-ui/protocol";
import { errorText, invariant, object, seq, string, type Json } from "../guards.ts";
import { DaemonError } from "../errors.ts";

/** Commands whose reply is followed by pushed frames on the same subscription. */
export const SUBSCRIPTIONS: ReadonlySet<string> = new Set(["subscribe", "subscribe_changes", "subscribe_feed"]);

/** The close code a failed bridge call ends the link with: the daemon may be gone. */
export const CLOSE_BRIDGE_FAILED = 1011;

type Subscription = { release?: () => void; ended: boolean; req: number; id?: number };

function record(value: unknown): Json | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Json : undefined;
}

function replyResult(frame: unknown, req: number): Json | undefined {
  const reply = record(frame);
  if (reply?.frame !== "reply" || reply.id !== req) return undefined;
  return record(reply.result);
}

class Subscriptions {
  private readonly pending = new Map<number, Subscription>();
  private readonly live = new Map<number, Subscription>();
  private readonly all = new Set<Subscription>();

  start(req: number): Subscription {
    invariant(req > 0 && !this.pending.has(req), "subscription request id is positive and unique");
    const sub: Subscription = { req, ended: false };
    this.pending.set(req, sub);
    this.all.add(sub);
    return sub;
  }

  receive(frame: unknown): void {
    const reply = record(frame);
    if (reply?.frame !== "reply" || typeof reply.id !== "number") return;
    const sub = this.pending.get(reply.id);
    if (!sub) return;
    const result = replyResult(frame, sub.req);
    if (!result) return;
    if (Object.hasOwn(result, "Err")) return this.end(sub);
    const ok = record(result.Ok);
    const data = record(ok?.data);
    if (ok?.reply !== "subscribed" || !Number.isSafeInteger(data?.id) || (data?.id as number) < 0) return;
    const id = data?.id as number;
    invariant(!this.live.has(id), "daemon subscription id is unique");
    this.pending.delete(sub.req);
    sub.id = id;
    this.live.set(id, sub);
  }

  attach(sub: Subscription, release: () => void): void {
    invariant(typeof release === "function" && sub.release === undefined, "subscription has one release callback");
    if (sub.ended) release();
    else sub.release = release;
  }

  find(id: number): Subscription | undefined {
    return this.live.get(id);
  }

  end(sub: Subscription): void {
    if (sub.ended) return;
    sub.ended = true;
    this.pending.delete(sub.req);
    if (sub.id !== undefined && this.live.get(sub.id) === sub) this.live.delete(sub.id);
    this.all.delete(sub);
    sub.release?.();
  }

  close(): void {
    for (const sub of this.all) this.end(sub);
  }
}

async function connect(bridge: DesktopBridge, target: string): Promise<string> {
  try {
    return string(await bridge.connect(string(target, "local daemon id")), "bridge connection id");
  } catch (error) {
    if (/\b(?:ENOENT|ECONNREFUSED)\b/.test(errorText(error))) throw new DaemonError("disconnected", "The local daemon is not connected", {}, { cause: error });
    throw error;
  }
}

/** `report` hears failures nothing waits on, such as closing a link already given up. */
export function ipcTransport(bridge: DesktopBridge, report: (message: string) => void): Transport {
  invariant(bridge.version === 1, `unsupported desktop bridge version ${String(bridge.version)}`);
  return {
    id: "ipc",
    async open(target, onFrame, onClose) {
      const conn = await connect(bridge, target);
      const subscriptions = new Subscriptions();
      let open = true;
      let removeCloseListener = () => {};
      const cleanup = () => { removeCloseListener(); subscriptions.close(); };
      const lost = (error: unknown) => {
        if (!open) return;
        open = false;
        cleanup();
        bridge.close(conn).catch((closeError: unknown) => report(`Could not close local connection ${conn}: ${errorText(closeError)}`));
        onClose(CLOSE_BRIDGE_FAILED, `Desktop bridge call failed: ${errorText(error)}`);
      };
      removeCloseListener = bridge.onClose(conn, () => lost(new Error("The daemon closed the connection")));
      const deliver = (frame: unknown) => {
        if (!open) return;
        subscriptions.receive(frame);
        onFrame(frame);
      };
      const rejected = (command: Json, error: unknown) => {
        const message = errorText(error);
        if (!message.startsWith("BridgeRefusal: ")) return lost(error);
        if (/\b(?:is closed|is not an open local connection|belongs to another window)\b/.test(message)) return lost(error);
        const id = seq(command.req, "command.req");
        invariant(id > 0, "a refused request has a positive id");
        deliver({ frame: "reply", id, result: { Err: { code: "forbidden", message: message.slice("BridgeRefusal: ".length) } } });
      };
      return {
        send(frame: unknown) {
          invariant(open, "the ipc link is open");
          const command: Json = object(frame, "client frame");
          const cmd = string(command.cmd, "command.cmd");
          if (SUBSCRIPTIONS.has(cmd)) {
            const sub = subscriptions.start(seq(command.req, "command.req"));
            bridge.subscribe(conn, command, deliver).then((unsubscribe) => {
              subscriptions.attach(sub, unsubscribe);
            }, (error: unknown) => { subscriptions.end(sub); rejected(command, error); });
          } else {
            const unsubscribed = cmd === "unsubscribe" ? subscriptions.find(seq(command.id, "command.id")) : undefined;
            bridge.request(conn, command).then((reply) => {
              if (cmd === "unsubscribe") {
                const result = replyResult(reply, seq(command.req, "command.req"));
                if (unsubscribed && result && Object.hasOwn(result, "Ok")) subscriptions.end(unsubscribed);
              }
              deliver(reply);
            }, (error: unknown) => rejected(command, error));
          }
        },
        close() {
          if (!open) return;
          open = false;
          cleanup();
          bridge.close(conn).catch((error: unknown) => report(`Could not close local connection ${conn}: ${errorText(error)}`));
        },
      };
    },
  };
}
