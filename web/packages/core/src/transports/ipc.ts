// The `ipc` transport over Electron main's bridge (CONTRACTS.md §2.4): requests
// map to bridge.request, subscriptions to bridge.subscribe; both answer frames.

import type { DesktopBridge, Transport } from "@aether-ui/protocol";
import { errorText, invariant, object, seq, string, type Json } from "../guards.ts";
import { DaemonError } from "../errors.ts";

/** Commands whose reply is followed by pushed frames on the same subscription. */
export const SUBSCRIPTIONS: ReadonlySet<string> = new Set(["subscribe", "subscribe_changes", "subscribe_feed"]);

/** The close code a failed bridge call ends the link with: the daemon may be gone. */
export const CLOSE_BRIDGE_FAILED = 1011;

/** `report` hears failures nothing waits on, such as closing a link already given up. */
export function ipcTransport(bridge: DesktopBridge, report: (message: string) => void): Transport {
  invariant(bridge.version === 1, `unsupported desktop bridge version ${String(bridge.version)}`);
  return {
    id: "ipc",
    async open(target, onFrame, onClose) {
      let connection: string;
      try {
        connection = await bridge.connect(string(target, "local daemon id"));
      } catch (error) {
        if (/\b(?:ENOENT|ECONNREFUSED)\b/.test(errorText(error))) throw new DaemonError("disconnected", "The local daemon is not connected", {}, { cause: error });
        throw error;
      }
      const conn = string(connection, "bridge connection id");
      const unsubscribes: Array<() => void> = [];
      let open = true;
      const lost = (error: unknown) => {
        if (!open) return;
        open = false;
        for (const unsubscribe of unsubscribes.splice(0)) unsubscribe();
        bridge.close(conn).catch((closeError: unknown) => report(`Could not close local connection ${conn}: ${errorText(closeError)}`));
        onClose(CLOSE_BRIDGE_FAILED, `Desktop bridge call failed: ${errorText(error)}`);
      };
      unsubscribes.push(bridge.onClose(conn, () => lost(new Error("The daemon closed the connection"))));
      const deliver = (frame: unknown) => { if (open) onFrame(frame); };
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
            bridge.subscribe(conn, command, deliver).then((unsubscribe) => {
              if (open) unsubscribes.push(unsubscribe);
              else unsubscribe();
            }, (error: unknown) => rejected(command, error));
          } else {
            bridge.request(conn, command).then(deliver, (error: unknown) => rejected(command, error));
          }
        },
        close() {
          if (!open) return;
          open = false;
          for (const unsubscribe of unsubscribes.splice(0)) unsubscribe();
          bridge.close(conn).catch((error: unknown) => report(`Could not close local connection ${conn}: ${errorText(error)}`));
        },
      };
    },
  };
}
