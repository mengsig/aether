// createApp(host): the store, the connection manager with the endpoints this
// host can reach, command sending with error reporting, and pairing flows.

import type { HostAdapter, PrivilegedAction } from "@aether-ui/protocol";
import type { Endpoint } from "./connection.ts";
import { codeOf, RefusedError } from "./errors.ts";
import { array, errorText, invariant, object, type Json } from "./guards.ts";
import { ConnectionManager } from "./manager.ts";
import { aetherPairing, archonPairing, PairingFlow } from "./pairing.ts";
import { applyRead, failRead, markRequested, settleSubmission } from "./reads.ts";
import { registry, type Registry } from "./registry.ts";
import { Store, INITIAL_STATE, type FrameScheduler } from "./store.ts";
import { indexedDbIdentity } from "./transports/identity.ts";
import { ipcTransport } from "./transports/ipc.ts";
import { ARCHON_MOUNT, archonSocketTransport } from "./transports/ws-archon.ts";
import { aetherSocketTransport } from "./transports/ws-aether.ts";

/** The web package version, sent in every hello. */
export const CLIENT_VERSION = "0.0.0";

/** A privileged action as a screen names it: core fills in the desktop's own connection id. */
export type LocalAction = PrivilegedAction extends infer A ? (A extends { conn: string } ? Omit<A, "conn"> : never) : never;

export interface AppOptions {
  /** Endpoints to use instead of the host's own discovery (tests, dev servers). */
  readonly endpoints?: readonly Endpoint[];
  readonly scheduler?: FrameScheduler;
  readonly fetch?: typeof fetch;
}

export interface App {
  readonly host: HostAdapter;
  readonly store: Store;
  readonly manager: ConnectionManager;
  readonly registry: Registry;
  /** Sends through the manager's checks; a refusal or daemon error is reported, then rethrown. */
  send(conn: string, command: Json): Promise<Json>;
  /**
   * Sends a read and stores its reply's `data` whole under (conn, key), unless a newer reread
   * already answered; a failure keeps the last value and stores the error. `tags` are the replies accepted.
   */
  reread(conn: string, key: string, command: Json, tags: readonly string[]): Promise<void>;
  /**
   * Runs a host-only action through the desktop's native confirmation (CONTRACTS.md §2.4), on a local
   * connection only; main builds the frame. Refused, and reported, anywhere else.
   */
  privileged(conn: string, action: LocalAction): Promise<unknown>;
  /** Drops a submission from the outbox and the review once the operator settled its outcome. */
  settle(conn: string, key: string): void;
  pairing(daemon: "aether" | "archon"): PairingFlow;
  start(): Promise<void>;
  stop(): void;
}

export function createApp(host: HostAdapter, options: AppOptions = {}): App {
  invariant(host.kind === "browser" || host.kind === "electron", `unknown host kind ${String(host.kind)}`);
  invariant(host.kind === "electron" || host.desktop === undefined, "a browser host has no desktop bridge");
  const store = new Store(INITIAL_STATE, options.scheduler);
  const manager = new ConnectionManager({ store, host, clientVersion: CLIENT_VERSION });
  const fetcher = options.fetch ?? globalThis.fetch?.bind(globalThis);
  let issued = 0;
  const app: App = {
    host, store, manager, registry,
    async send(conn, command) {
      try {
        return await manager.send(conn, command);
      } catch (error) {
        store.report(errorText(error), conn, codeOf(error));
        throw error;
      }
    },
    async reread(conn, key, command, tags) {
      invariant(key.length > 0 && tags.length > 0, "a reread names its store key and the replies it accepts");
      issued += 1;
      const order = issued;
      store.update((state) => markRequested(state, conn, key, order));
      try {
        const reply = await app.send(conn, command);
        if (!tags.includes(String(reply.reply))) throw new Error(`Expected a ${tags.join(" or ")} reply, got ${String(reply.reply)}`);
        store.update((state) => applyRead(state, conn, key, order, reply.data ?? null));
      } catch (error) {
        store.update((state) => failRead(state, conn, key, order, errorText(error) || "The read failed"));
      }
    },
    async privileged(conn, action) {
      try {
        return await runPrivileged(host, manager, conn, action, (message) => store.report(message, conn));
      } catch (error) {
        store.report(errorText(error), conn, codeOf(error));
        throw error;
      }
    },
    settle(conn, key) {
      store.update((state) => settleSubmission(state, conn, key));
    },
    pairing(daemon) {
      const retry = () => { for (const c of manager.all()) if (c.endpoint.daemon === daemon) c.retryNow(); };
      if (daemon === "archon") return new PairingFlow(archonPairing(fetcher), retry);
      return new PairingFlow(aetherPairing(indexedDbIdentity(globalThis.location.origin), fetcher), retry);
    },
    async start() {
      const endpoints = options.endpoints ?? (await discover(host, store, fetcher));
      for (const endpoint of endpoints) manager.add(endpoint);
      if (host.desktop) manager.setManagedStart((await host.desktop.getPreference("desktop.managed_start")) === true);
      manager.start();
    },
    stop: () => manager.stop(),
  };
  return app;
}

async function runPrivileged(host: HostAdapter, manager: ConnectionManager, conn: string, action: LocalAction, report: (message: string) => void): Promise<unknown> {
  invariant(typeof action.kind === "string", "a privileged action names its kind");
  const desktop = host.desktop;
  if (!desktop) throw new RefusedError("Only the desktop app on the daemon's own host can do this");
  const connection = manager.get(conn);
  if (!connection || connection.endpoint.transport.id !== "ipc" || connection.current.standing !== "local") {
    throw new RefusedError("Only the daemon's own host can do this");
  }
  // The bridge names connections its own way; a short-lived one carries just this action.
  const local = await desktop.connect(connection.endpoint.target);
  invariant(typeof local === "string" && local.length > 0, "the bridge returns a connection id");
  try {
    return await desktop.privileged({ ...action, conn: local } as PrivilegedAction);
  } finally {
    desktop.close(local).catch((error: unknown) => report(`Could not close local connection ${local}: ${errorText(error)}`));
  }
}

async function discover(host: HostAdapter, store: Store, fetcher: typeof fetch): Promise<Endpoint[]> {
  if (host.desktop) {
    const transport = ipcTransport(host.desktop, (message) => store.report(message));
    const daemons = await host.desktop.discover();
    return daemons.filter((d) => d.reachable).map((d) => ({ daemon: d.kind, transport, target: d.id, label: d.label }));
  }
  const { origin, host: hostLabel, protocol } = globalThis.location;
  const socketOrigin = `${protocol === "https:" ? "wss:" : "ws:"}//${hostLabel}`;
  const endpoints: Endpoint[] = [{
    daemon: "aether", label: hostLabel, target: `${socketOrigin}/ws`,
    transport: aetherSocketTransport({ identity: indexedDbIdentity(origin), origin }),
  }];
  if (await archonMounted(fetcher)) {
    endpoints.push({ daemon: "archon", label: hostLabel, target: `${socketOrigin}${ARCHON_MOUNT}/ws`, transport: archonSocketTransport({ fetch: fetcher }) });
  }
  return endpoints;
}

/** Whether Aether's health lists an Archon mount at /archon (CONTRACTS.md §1.1). */
export async function archonMounted(fetcher: typeof fetch): Promise<boolean> {
  const response = await fetcher("/api/health", { credentials: "omit", cache: "no-store" });
  if (!response.ok) throw new Error(`Aether's health check answered ${response.status}`);
  const health = object(await response.json(), "health");
  const mounts = array(health.mounts ?? [], "health.mounts");
  return mounts.some((mount) => object(mount, "mount").prefix === ARCHON_MOUNT && object(mount, "mount").kind === "archon");
}
