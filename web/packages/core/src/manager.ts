// ConnectionManager: every daemon connection of the app, the commands sent
// through them, and the Capabilities they add up to.

import type { ConnectionInfo, ConnId, HostAdapter } from "@aether-ui/protocol";
import type { ArchonSync } from "./archon-sync.ts";
import { computeCapabilities, sameCapabilities } from "./capabilities.ts";
import { aetherAccess, archonAccess, checkAllowed, withIdempotencyKey } from "./commands.ts";
import { Connection, type Endpoint } from "./connection.ts";
import { codeOf, DaemonError, RefusedError } from "./errors.ts";
import { errorText, invariant, omit, string, type Json } from "./guards.ts";
import { nextTicket } from "./noesis-cache.ts";
import { installNoesisChange, installNoesisRead, noesisReadCommand, type NoesisReadRequest } from "./noesis-reads.ts";
import { dropConnection, type State, type Store, type Submission } from "./store.ts";

export interface ManagerOptions {
  readonly store: Store;
  readonly host: HostAdapter;
  readonly clientVersion: string;
  readonly managedStart?: boolean;
  readonly random?: () => number;
}

/** Errors after which a sent command may or may not have acted; any other is its own answer. */
const UNKNOWN_OUTCOME = new Set(["timeout", "disconnected"]);

export class ConnectionManager {
  private readonly options: ManagerOptions;
  private readonly connections: Connection[] = [];
  private managedStart: boolean;

  constructor(options: ManagerOptions) {
    this.options = options;
    this.managedStart = options.managedStart === true;
    this.refreshCapabilities();
  }

  /** The `desktop.managed_start` preference changed. */
  setManagedStart(enabled: boolean): void {
    this.managedStart = enabled;
    this.refreshCapabilities();
  }

  add(endpoint: Endpoint): Connection {
    invariant(!this.connections.some((c) => c.endpoint.daemon === endpoint.daemon && c.endpoint.target === endpoint.target), `${endpoint.target} is added once`);
    const connection = new Connection(endpoint, {
      store: this.options.store,
      clientVersion: this.options.clientVersion,
      ...(this.options.random ? { random: this.options.random } : {}),
      publish: (info, previous) => this.publish(info, previous),
      settingsChanged: () => this.refreshCapabilities(),
    });
    this.connections.push(connection);
    return connection;
  }

  start(): void {
    for (const connection of this.connections) connection.start();
  }

  stop(): void {
    for (const connection of this.connections) connection.stop();
  }

  get(conn: ConnId): Connection | undefined {
    return this.connections.find((connection) => connection.id === conn);
  }

  all(): readonly Connection[] {
    return this.connections;
  }

  /**
   * Sends a command after checking the connection's standing; a keyed
   * mutation gets an idempotency key and sits in the outbox until answered.
   */
  async send(conn: ConnId, input: Json): Promise<Json> {
    const connection = this.get(conn);
    if (!connection) throw new RefusedError(`No connection ${conn}`);
    const aether = connection.endpoint.daemon === "aether";
    const access = aether ? aetherAccess(input) : archonAccess(input);
    checkAllowed(access, connection.current.standing);
    const command = aether ? withIdempotencyKey(input) : input;
    const key = typeof command.idempotency_key === "string" ? command.idempotency_key : null;
    const held = key ? this.heldSubmission(conn, key) : undefined;
    const horizon = held === undefined ? connection.horizon : held.horizon ?? null;
    const attempt = (held?.attempt ?? 0) + 1;
    const submission = { conn, key: key ?? "", command, attempt, ...(key && horizon !== null ? { horizon } : {}) };
    if (key) this.track(conn, aether, { ...submission, state: "pending" });
    try {
      const reply = await connection.request({ ...command, cmd: string(command.cmd, "command.cmd") });
      if (key) this.untrack(conn, aether, key);
      return reply;
    } catch (error) {
      if (key && UNKNOWN_OUTCOME.has(codeOf(error) ?? "")) this.track(conn, aether, { ...submission, state: "unknown", error: errorText(error) });
      else if (key) this.untrack(conn, aether, key);
      throw error;
    }
  }

  /**
   * The earlier send under this key, if any: a resend keeps its horizon (a later one would vouch for
   * dropped receipts) and counts one more attempt.
   */
  private heldSubmission(conn: ConnId, key: string): Submission | undefined {
    invariant(key !== "", "only keyed submissions are held");
    const state = this.options.store.get();
    const held = [...(state.aether[conn]?.outbox ?? []), ...state.review.filter((s) => s.conn === conn)].find((s) => s.key === key);
    invariant(held === undefined || held.command.idempotency_key === key, "a held submission carries its own key");
    return held;
  }

  private track(conn: ConnId, aether: boolean, submission: Submission): void {
    const put = <T extends { outbox: readonly Submission[] }>(s: T): T => ({ ...s, outbox: [...s.outbox.filter((x) => x.key !== submission.key), submission] });
    if (aether) this.options.store.updateAether(conn, put);
    else this.options.store.updateArchon(conn, put);
  }

  private untrack(conn: ConnId, aether: boolean, key: string): void {
    const drop = <T extends { outbox: readonly Submission[] }>(s: T): T => (s.outbox.some((x) => x.key === key) ? { ...s, outbox: s.outbox.filter((x) => x.key !== key) } : s);
    if (aether) this.options.store.updateAether(conn, drop);
    else this.options.store.updateArchon(conn, drop);
  }

  /** §5.2.2: the agents on screen for one Aether connection; resumed after reconnects. */
  watchAgents(conn: ConnId, agents: readonly string[]): Promise<void> {
    const connection = this.get(conn);
    if (connection?.endpoint.daemon !== "aether") return Promise.reject(new DaemonError("not_found", `${conn} is not an Aether connection`));
    return connection.show(agents);
  }

  /** §5.3.3: the items on screen for one archond connection; resumed after reconnects. */
  showItems(conn: ConnId, items: readonly string[]): void {
    const connection = this.get(conn);
    invariant(connection?.endpoint.daemon === "archon", `${conn} is not an archond connection`);
    void connection.show(items);
  }

  /** Rereads one item's detail, links and plans into the store; the caller sees the failure. */
  refreshItem(conn: ConnId, item: string): Promise<void> {
    return this.archonSync(conn).refresh(item);
  }

  /** Reads one run's plan into the store; the item's rereads keep it current. */
  readPlan(conn: ConnId, item: string, run: string): Promise<void> {
    return this.archonSync(conn).readPlan(item, run);
  }

  /** Reads one Noesis entity into the store (noesis-reads.ts); the caller sees the failure. */
  async readNoesis(conn: ConnId, read: NoesisReadRequest): Promise<void> {
    const ticket = this.noesisTicket(conn);
    const reply = await this.send(conn, noesisReadCommand(read));
    this.options.store.updateAether(conn, (s) => installNoesisRead(s, read, reply, ticket));
    if (read.kind === "status") this.refreshCapabilities();
  }

  /** Lands the reply of a Noesis change a screen sent through useCommand, so the screen derives it from the store. */
  noesisChanged(conn: ConnId, command: Json, reply: Json): void {
    const ticket = this.noesisTicket(conn);
    this.options.store.updateAether(conn, (s) => installNoesisChange(s, command, reply, ticket));
    if (command.cmd === "noesis_configure") this.refreshCapabilities();
  }

  /** Hands out the connection's next Noesis read ticket from the store, so it keeps growing across reconnects. */
  private noesisTicket(conn: ConnId): number {
    invariant(this.get(conn)?.endpoint.daemon === "aether", `${conn} is not an Aether connection`);
    let ticket = 0;
    this.options.store.updateAether(conn, (s) => {
      const learn = nextTicket(s.learn);
      ticket = learn.ticket;
      return { ...s, learn };
    });
    invariant(ticket > 0, "a ticket was handed out");
    return ticket;
  }

  private archonSync(conn: ConnId): ArchonSync {
    const connection = this.get(conn);
    if (connection?.endpoint.daemon !== "archon") throw new DaemonError("not_found", `${conn} is not an archond connection`);
    const sync = connection.archonSync;
    if (sync === null) throw new DaemonError("not_connected", `${conn} is not connected`);
    return sync;
  }

  private publish(info: ConnectionInfo, previous?: ConnId): void {
    this.options.store.update((state) => {
      let next: State = state;
      if (previous !== undefined && previous !== info.id) {
        // §5.1: a new instance_id drops the old id's cache; its unconfirmed submissions go to review.
        next = dropConnection(next, previous);
        next = { ...next, connections: omit(next.connections, previous) };
      }
      return { ...next, connections: { ...next.connections, [info.id]: info } };
    });
    this.refreshCapabilities();
  }

  refreshCapabilities(): void {
    const { host } = this.options;
    this.options.store.update((state) => {
      const noesis = Object.fromEntries(Object.entries(state.aether).map(([id, s]) => [id, s.noesis]));
      const capabilities = computeCapabilities({
        host: host.kind, desktop: host.desktop !== undefined, managedStart: this.managedStart,
        connections: state.connections, noesis,
      });
      return sameCapabilities(capabilities, state.capabilities) ? state : { ...state, capabilities };
    });
  }
}
