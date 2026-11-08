// createApp(host): the store, the connection manager with the endpoints this
// host can reach, command sending with error reporting, and pairing flows.

import type { HostAdapter, PrivilegedAction } from "@aether-ui/protocol";
import { isKeyed } from "./commands.ts";
import type { Endpoint } from "./connection.ts";
import { codeOf, RefusedError } from "./errors.ts";
import { array, errorText, invariant, object, type Json } from "./guards.ts";
import { ConnectionManager } from "./manager.ts";
import { aetherPairing, archonPairing, PairingFlow } from "./pairing.ts";
import { applyRead, failRead, markRequested, settleSubmission } from "./reads.ts";
import { registry, type Registry } from "./registry.ts";
import { Store, INITIAL_STATE, type FrameScheduler, type Submission } from "./store.ts";
import { indexedDbIdentity, type IdentityStore } from "./transports/identity.ts";
import { ipcTransport } from "./transports/ipc.ts";
import { ARCHON_MOUNT, archonSocketTransport } from "./transports/ws-archon.ts";
import { aetherSocketTransport } from "./transports/ws-aether.ts";

/** The web package version, sent in every hello. */
export const CLIENT_VERSION = "1.3.0";

/** A privileged action as a screen names it: core fills in the desktop's own connection id. */
export type LocalAction = PrivilegedAction extends infer A ? (A extends { conn: string } ? Omit<A, "conn"> : never) : never;

export interface AppOptions {
  /** Endpoints to use instead of the host's own discovery (tests, dev servers). */
  readonly endpoints?: readonly Endpoint[];
  readonly scheduler?: FrameScheduler;
  readonly fetch?: typeof fetch;
  /** Where this browser keeps its Aether device identity; IndexedDB for the page's origin by default. */
  readonly identities?: IdentityStore;
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
  /**
   * Removes this browser's saved Aether device identity and reconnects, which then asks to pair.
   * The daemon keeps the device until it is revoked there.
   */
  forgetDevice(conn: string): Promise<void>;
  /** Puts an unkeyed change whose reply was lost in the recovery review; returns its local key. */
  noteUnconfirmed(conn: string, command: Json, error: string): string;
  pairing(daemon: "aether" | "archon"): PairingFlow;
  start(): Promise<void>;
  /**
   * Desktop only: asks the bridge again which local daemons answer, connects any new one and
   * retries the rest, for daemons started after the app (first-run setup). Returns how many were added.
   */
  rediscover(): Promise<number>;
  stop(): void;
}

export function createApp(host: HostAdapter, options: AppOptions = {}): App {
  invariant(host.kind === "browser" || host.kind === "electron", `unknown host kind ${String(host.kind)}`);
  invariant(host.kind === "electron" || host.desktop === undefined, "a browser host has no desktop bridge");
  const store = new Store(INITIAL_STATE, options.scheduler);
  const manager = new ConnectionManager({ store, host, clientVersion: CLIENT_VERSION });
  const fetcher = options.fetch ?? globalThis.fetch?.bind(globalThis);
  const identities = () => options.identities ?? indexedDbIdentity(globalThis.location.origin);
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
    async forgetDevice(conn) {
      const connection = manager.get(conn);
      invariant(connection !== undefined, `connection ${conn} is known to the manager`);
      if (connection.endpoint.transport.id !== "ws-aether") throw new RefusedError("Only a browser paired with an Aether daemon has a saved device to forget");
      // Close the signed-in link first: it must not outlive the identity it authenticated.
      connection.stop();
      try {
        await identities().remove();
      } finally {
        connection.start();
      }
    },
    noteUnconfirmed(conn, command, error) {
      invariant(typeof command.cmd === "string" && !isKeyed(command), "only an unkeyed change is noted here; core tracks keyed ones");
      invariant(store.get().connections[conn]?.daemon === "aether", "lost changes are noted on an Aether connection");
      const key = `unkeyed:${crypto.randomUUID()}`;
      const submission: Submission = { conn, key, command, state: "unknown", error, unkeyed: true };
      store.updateAether(conn, (s) => ({ ...s, outbox: [...s.outbox, submission] }));
      return key;
    },
    pairing(daemon) {
      const retry = () => { for (const c of manager.all()) if (c.endpoint.daemon === daemon) c.retryNow(); };
      if (daemon === "archon") return new PairingFlow(archonPairing(fetcher), retry);
      return new PairingFlow(aetherPairing(identities(), fetcher), retry);
    },
    async start() {
      const endpoints = options.endpoints ?? (await discover(host, store, fetcher, identities));
      for (const endpoint of endpoints) manager.add(endpoint);
      const managed = host.desktop !== undefined && (await host.desktop.getPreference("desktop.managed_start")) === true;
      if (host.desktop) manager.setManagedStart(managed);
      manager.start();
      if (managed && options.endpoints === undefined) void startMissing(app);
    },
    async rediscover() {
      if (!host.desktop) throw new RefusedError("Only the desktop app finds daemons on this computer");
      const found = await discover(host, store, fetcher, identities);
      const known = (endpoint: Endpoint) => manager.all().some((c) => c.endpoint.daemon === endpoint.daemon && c.endpoint.target === endpoint.target);
      const fresh = found.filter((endpoint) => !known(endpoint));
      for (const connection of manager.all()) if (connection.current.state === "backoff") connection.retryNow();
      for (const endpoint of fresh) manager.add(endpoint).start();
      invariant(found.every(known), "every reachable daemon has a connection after rediscovery");
      return fresh.length;
    },
    stop: () => manager.stop(),
  };
  return app;
}

/** With managed start on, starts each local daemon that did not answer at launch, then connects it. */
async function startMissing(app: App): Promise<void> {
  const desktop = app.host.desktop;
  invariant(desktop !== undefined, "only the desktop app starts daemons");
  const missing = (["aether", "archon"] as const).filter((kind) => !app.manager.all().some((c) => c.endpoint.daemon === kind));
  if (missing.length === 0) return;
  let started = false;
  for (const kind of missing) {
    try {
      await desktop.startDaemon(kind);
      started = true;
    } catch (error) {
      app.store.report(`Could not start ${kind === "aether" ? "Aether" : "Archon"} on this computer: ${errorText(error)}`);
    }
  }
  if (!started) return;
  try {
    await app.rediscover();
  } catch (error) {
    app.store.report(`Could not reconnect after starting: ${errorText(error)}`);
  }
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

async function discover(host: HostAdapter, store: Store, fetcher: typeof fetch, identities: () => IdentityStore): Promise<Endpoint[]> {
  if (host.desktop) {
    const transport = ipcTransport(host.desktop, (message) => store.report(message));
    const daemons = await host.desktop.discover();
    return daemons.filter((d) => d.reachable).map((d) => ({ daemon: d.kind, transport, target: d.id, label: d.label }));
  }
  const { origin, host: hostLabel, protocol } = globalThis.location;
  const socketOrigin = `${protocol === "https:" ? "wss:" : "ws:"}//${hostLabel}`;
  const endpoints: Endpoint[] = [{
    daemon: "aether", label: hostLabel, target: `${socketOrigin}/ws`,
    transport: aetherSocketTransport({ identity: identities(), origin }),
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
