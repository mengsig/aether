// One daemon connection's lifecycle (CONTRACTS.md §2.5): open, greet, read the
// welcome, run its sync, and back off and reopen when the link drops.

import { aether, type ConnectionInfo, type ConnId, type ConnState, type Standing, type Transport } from "@aether-ui/protocol";
import { AetherSync, type SyncContext } from "./aether-sync.ts";
import { ArchonSync } from "./archon-sync.ts";
import { backoffDelay } from "./backoff.ts";
import { codeOf, DaemonError } from "./errors.ts";
import { errorText, invariant, object, seq, string, strings, type Json } from "./guards.ts";
import { replyObject } from "./replies.ts";
import { Rpc, type Command, type Link } from "./rpc.ts";
import type { Store } from "./store.ts";

export interface Endpoint {
  readonly daemon: "aether" | "archon";
  readonly transport: Transport;
  readonly target: string;
  /** The host label: names an archond connection and titles both kinds. */
  readonly label: string;
}

export interface ConnectionHooks {
  readonly store: Store;
  readonly clientVersion: string;
  readonly random?: () => number;
  /** A connection's info changed; `previous` is the id it had before, if different. */
  publish(info: ConnectionInfo, previous?: ConnId): void;
  settingsChanged(): void;
}

/** Hello versions per daemon, newest first. archond 24 accepts archon-web 23..=24; 23 is exact. */
export const ARCHON_VERSIONS = [24, 23] as const;
export const AETHER_HELLO_FEATURES = ["deltas", "artifact_bytes", "delta_order", "memory_v2"];
const CLOSE_REVOKED = 4401;
const CLOSE_BUSY = 4503;
const CLOSE_LAGGED = 1013;
const PAIRING_CODES = new Set(["unpaired", "identity_changed", "unauthorized", "revoked"]);

export class Connection {
  readonly endpoint: Endpoint;
  private readonly hooks: ConnectionHooks;
  private info: ConnectionInfo;
  private rpc: Rpc | null = null;
  private link: Link | null = null;
  private sync: AetherSync | ArchonSync | null = null;
  private attempt = 0;
  private versionIndex = 0;
  private generation = 0;
  private stopped = true;
  private retryTimer: ReturnType<typeof setTimeout> | undefined;
  private heartbeat: ReturnType<typeof setInterval> | undefined;
  /** What the screen shows; kept across reconnects so each new sync resumes it. */
  private onScreen: readonly string[] = [];
  /** The welcome's `submission_horizon`, which a lost keyed command's resolution needs. */
  private submissionHorizon: string | null = null;

  constructor(endpoint: Endpoint, hooks: ConnectionHooks) {
    string(endpoint.target, "endpoint target");
    string(endpoint.label, "endpoint label");
    this.endpoint = endpoint;
    this.hooks = hooks;
    const id = endpoint.daemon === "archon" ? `archon:${endpoint.label}` : `aether:@${endpoint.label}`;
    this.info = { id, daemon: endpoint.daemon, state: "closed", features: [], label: endpoint.label };
  }

  get id(): ConnId {
    return this.info.id;
  }

  get current(): ConnectionInfo {
    return this.info;
  }

  get horizon(): string | null {
    return this.submissionHorizon;
  }

  get aetherSync(): AetherSync | null {
    return this.sync instanceof AetherSync ? this.sync : null;
  }

  get archonSync(): ArchonSync | null {
    return this.sync instanceof ArchonSync ? this.sync : null;
  }

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    void this.open();
  }

  stop(): void {
    this.stopped = true;
    this.generation += 1;
    clearTimeout(this.retryTimer);
    this.teardown(new DaemonError("disconnected", "The app closed the connection"));
    this.setState("closed");
  }

  /** Skips a backoff wait, or leaves `pairing` after the device paired. */
  retryNow(): void {
    if (this.stopped || this.info.state === "ready" || this.info.state === "connecting") return;
    clearTimeout(this.retryTimer);
    void this.open();
  }

  /** §5.2.2 and §5.3.3: the agents or items on screen, applied now and after every reconnect. */
  show(ids: readonly string[]): Promise<void> {
    this.onScreen = ids.map((id) => string(id, "on-screen id"));
    if (this.sync instanceof AetherSync) return this.sync.watch(this.onScreen);
    this.archonSync?.show(this.onScreen);
    return Promise.resolve();
  }

  request(command: Command): Promise<Json> {
    if (this.info.state !== "ready" || !this.rpc) return Promise.reject(new DaemonError("not_connected", `${this.info.label} is not connected`));
    return this.rpc.request(command);
  }

  private async open(): Promise<void> {
    const generation = ++this.generation;
    this.setState("connecting");
    try {
      const rpcBox: { rpc?: Rpc } = {};
      const link = await this.endpoint.transport.open(this.endpoint.target,
        (frame) => { if (generation === this.generation) this.receive(frame, rpcBox.rpc); },
        (code, reason) => { if (generation === this.generation) this.closed(code, reason); });
      if (generation !== this.generation) return link.close();
      this.link = link;
      this.rpc = rpcBox.rpc = new Rpc(link);
      const welcome = await this.greet(this.rpc);
      if (generation !== this.generation) return;
      this.ready(welcome);
      await this.sync?.start();
    } catch (error) {
      if (generation === this.generation) this.failed(error);
    }
  }

  private greet(rpc: Rpc): Promise<Json> {
    if (this.endpoint.daemon === "aether") {
      return rpc.request(aether.hello({ client_name: "aether-web", client_version: this.hooks.clientVersion,
        protocol_version: aether.PROTOCOL_VERSION, features: AETHER_HELLO_FEATURES })).then((r) => replyObject(r, "welcome"));
    }
    const version = ARCHON_VERSIONS[this.versionIndex] ?? 23;
    const hello = { cmd: "hello", protocol_version: version, client: "archon-web", client_version: this.hooks.clientVersion };
    return rpc.request(version >= 24 ? { ...hello, features: [] } : hello).then((r) => replyObject(r, "welcome"));
  }

  /** Reads the welcome into the connection's id, protocol, features and standing. */
  private ready(welcome: Json): void {
    const protocol = seq(welcome.protocol_version, "welcome.protocol_version");
    const features = strings(welcome.features, "welcome.features");
    let id = this.info.id;
    if (this.endpoint.daemon === "aether") {
      invariant(protocol >= aether.MIN_PROTOCOL_VERSION && protocol <= aether.PROTOCOL_VERSION, `Aether protocol ${protocol} is outside this app's range`);
      id = `aether:${string(welcome.instance_id, "welcome.instance_id")}`;
    } else {
      invariant((ARCHON_VERSIONS as readonly number[]).includes(protocol), `Archon protocol ${protocol} is outside this app's range`);
    }
    this.attempt = 0;
    const previous = this.info.id;
    // Entities of a replaced daemon instance do not exist on the new one.
    if (previous !== id && this.info.protocol !== undefined) this.onScreen = [];
    const horizon = welcome.submission_horizon;
    this.submissionHorizon = typeof horizon === "string" && horizon !== "" ? horizon : null;
    this.info = { ...this.info, id, protocol, features, standing: this.standing(welcome), state: "ready" };
    const ctx = this.syncContext(new Set(features));
    this.sync = this.endpoint.daemon === "aether" ? new AetherSync(ctx) : new ArchonSync(ctx);
    void this.show(this.onScreen);
    this.hooks.publish(this.info, previous === id ? undefined : previous);
    this.startHeartbeat(welcome.idle_timeout_secs);
  }

  private standing(welcome: Json): Standing {
    if (this.endpoint.transport.id === "ipc") return "local";
    if (welcome.device == null && this.endpoint.daemon === "archon") return "operator"; // protocol 23 devices are operators
    const role = object(welcome.device, "welcome.device").role;
    if (role !== "operator" && role !== "observer") throw new DaemonError("internal", `Invalid device role ${String(role)}`);
    return role;
  }

  private syncContext(features: ReadonlySet<string>): SyncContext {
    return {
      conn: this.info.id, store: this.hooks.store, features,
      request: (command) => this.request(command),
      settingsChanged: () => this.hooks.settingsChanged(),
    };
  }

  private receive(raw: unknown, rpc: Rpc | undefined): void {
    try {
      const frame = object(raw, "server frame");
      if (frame.frame === "reply") {
        const diagnostic = rpc?.settle(frame);
        if (diagnostic) this.failed(diagnostic);
      } else if (frame.frame === "event") this.aetherSync?.onEvent(frame);
      else if (frame.frame === "delta") this.aetherSync?.onDelta(frame);
      else if (frame.frame === "change") this.archonSync?.onChange(frame);
      else if (frame.frame === "feed") this.archonSync?.onFeed(frame);
      else throw new Error(`Unknown server frame ${String(frame.frame)}`);
    } catch (error) {
      this.hooks.store.report(`Protocol error from ${this.info.label}: ${errorText(error)}`, this.info.id, "protocol");
      this.link?.close();
      this.closed(1002, errorText(error));
    }
  }

  private closed(code: number, reason: string): void {
    if (this.stopped) return;
    if (code === CLOSE_REVOKED) return this.failed(new DaemonError("revoked", reason || "Device access was refused or revoked"));
    if (code === CLOSE_BUSY) this.attempt = Math.max(this.attempt, 6);
    if (code === CLOSE_LAGGED) this.attempt = 0;
    this.failed(new DaemonError("disconnected", reason || `Connection closed (${code})`));
  }

  private failed(error: unknown): void {
    this.generation += 1;
    this.teardown(error instanceof Error ? error : new Error(errorText(error)));
    if (this.stopped) return;
    const code = codeOf(error) ?? "";
    if (PAIRING_CODES.has(code)) return this.setState("pairing");
    const fallBack = code === "incompatible" && this.endpoint.daemon === "archon" && this.versionIndex < ARCHON_VERSIONS.length - 1;
    if (fallBack) {
      // An older archond greets only its own version: retry with the next one down.
      this.versionIndex += 1;
      this.attempt = 0;
    } else if (code === "incompatible") {
      this.hooks.store.report(errorText(error), this.info.id, code);
      return this.setState("closed");
    }
    if (code !== "disconnected" && !fallBack) this.hooks.store.report(`${this.info.label}: ${errorText(error)}`, this.info.id, code || undefined);
    const delay = backoffDelay(this.attempt, this.hooks.random);
    this.attempt += 1;
    this.setState("backoff");
    this.retryTimer = setTimeout(() => void this.open(), delay);
  }

  private teardown(error: Error): void {
    clearInterval(this.heartbeat);
    this.sync?.stop();
    this.sync = null;
    this.rpc?.fail(error);
    this.rpc = null;
    this.link?.close();
    this.link = null;
  }

  private startHeartbeat(idle: unknown): void {
    clearInterval(this.heartbeat);
    if (idle == null) return;
    const seconds = seq(idle, "welcome.idle_timeout_secs");
    invariant(seconds >= 1, "an idle timeout is at least a second");
    const generation = this.generation;
    this.heartbeat = setInterval(() => {
      this.request(aether.ping()).catch((error: unknown) => {
        // A ping failed by the teardown of its own link is already handled.
        if (generation === this.generation) this.failed(error);
      });
    }, Math.max(250, Math.min(20_000, seconds * 500)));
  }

  private setState(state: ConnState): void {
    if (this.info.state === state) return;
    this.info = { ...this.info, state };
    this.hooks.publish(this.info);
  }
}
