// The `ws-archon` transport: archond behind Aether's `/archon` mount
// (ARCHITECTURE.md §d). The device cookie is scoped to `/archon` and rides
// on the same-origin upgrade; the session endpoint says whether one exists.

import type { Transport } from "@aether-ui/protocol";
import { DaemonError } from "../errors.ts";
import { object, type Json } from "../guards.ts";
import { browserSocket, checkSocketUrl, openSocket, parseFrame, sendFrame, type SocketFactory } from "./socket.ts";

export const ARCHON_MOUNT = "/archon";

export interface ArchonSocketOptions {
  readonly socket?: SocketFactory;
  readonly fetch?: typeof fetch;
}

/** GET /archon/api/session: the paired device, or `unpaired` on 401. */
export async function archonSession(fetcher: typeof fetch, socketUrl: string): Promise<Json> {
  const url = new URL(`${ARCHON_MOUNT}/api/session`, socketUrl.replace(/^ws/, "http"));
  const response = await fetcher(url.href, { method: "GET", credentials: "same-origin", cache: "no-store" });
  if (response.status === 401) throw new DaemonError("unpaired", "Pair this browser with Archon");
  if (!response.ok) throw new DaemonError("unavailable", `Archon's session check answered ${response.status}`);
  const session = object(await response.json(), "archon session");
  object(session.device, "archon session.device");
  return session;
}

export function archonSocketTransport(options: ArchonSocketOptions = {}): Transport {
  const factory = options.socket ?? browserSocket;
  return {
    id: "ws-archon",
    async open(target, onFrame, onClose) {
      const url = checkSocketUrl(target, `${ARCHON_MOUNT}/ws`);
      await archonSession(options.fetch ?? globalThis.fetch.bind(globalThis), url);
      const socket = await openSocket(factory, url, "archon", {
        onText(raw) {
          try {
            onFrame(parseFrame(raw));
          } catch (error) {
            socket.close(1002, "Invalid server frame");
            onClose(1002, `Protocol error: ${error instanceof Error ? error.message : String(error)}`);
          }
        },
        onClose,
      });
      return {
        send: (frame: unknown) => sendFrame(socket, object(frame, "client frame")),
        close: () => socket.close(1000, "bye"),
      };
    },
  };
}
