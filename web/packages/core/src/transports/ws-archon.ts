// The `ws-archon` transport: archond behind Aether's `/archon` mount. Its device
// cookie rides on the same-origin upgrade; /archon/api/session says if one exists.

import type { Transport } from "@aether-ui/protocol";
import { DaemonError } from "../errors.ts";
import { object, type Json } from "../guards.ts";
import { browserSocket, checkSocketUrl, openSocket, parseFrame, sendFrame, type SocketFactory } from "./socket.ts";

export const ARCHON_MOUNT = "/archon";

export interface ArchonSocketOptions {
  readonly socket?: SocketFactory;
  readonly fetch?: typeof fetch;
}

/**
 * GET /archon/api/session: the paired device, `unpaired` on 401, or null from
 * an archond too old to have the endpoint, whose upgrade then decides.
 */
export async function archonSession(fetcher: typeof fetch, socketUrl: string, signal?: AbortSignal): Promise<Json | null> {
  const request = async () => {
    const url = new URL(`${ARCHON_MOUNT}/api/session`, socketUrl.replace(/^ws/, "http"));
    const response = await fetcher(url.href, { method: "GET", credentials: "same-origin", cache: "no-store", signal: signal ?? null });
    if (response.status === 401) throw new DaemonError("unpaired", "Pair this browser with Archon");
    if (response.status === 404) return null;
    if (!response.ok) throw new DaemonError("unavailable", `Archon's session check answered ${response.status}`);
    const session = object(await response.json(), "archon session");
    object(session.device, "archon session.device");
    return session;
  };
  if (!signal) return request();
  signal.throwIfAborted();
  let abort: () => void = () => {};
  const cancelled = new Promise<never>((_, reject) => {
    abort = () => reject(new DOMException("Connection attempt cancelled", "AbortError"));
    signal.addEventListener("abort", abort, { once: true });
  });
  try { return await Promise.race([request(), cancelled]); }
  finally { signal.removeEventListener("abort", abort); }
}

export function archonSocketTransport(options: ArchonSocketOptions = {}): Transport {
  const factory = options.socket ?? browserSocket;
  return {
    id: "ws-archon",
    async open(target, onFrame, onClose, signal) {
      const url = checkSocketUrl(target, `${ARCHON_MOUNT}/ws`);
      await archonSession(options.fetch ?? globalThis.fetch.bind(globalThis), url, signal);
      signal?.throwIfAborted();
      const socket = await openSocket(factory, url, "archon", {
        onText(raw) {
          try {
            onFrame(parseFrame(raw));
          } catch (error) {
            // Browsers may send only 1000 or 3000-4999; the app still hears 1002.
            socket.close(1000, "Invalid server frame");
            onClose(1002, `Protocol error: ${error instanceof Error ? error.message : String(error)}`);
          }
        },
        onClose,
      }, signal);
      return {
        send: (frame: unknown) => sendFrame(socket, object(frame, "client frame")),
        close: () => socket.close(1000, "bye"),
      };
    },
  };
}
