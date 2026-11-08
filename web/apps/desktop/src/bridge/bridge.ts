// The bridge's main side: every IPC call enters through `handle`, passes the Validator, and
// only then reaches the method that serves it. Electron-specific effects come in through BridgeHost.

import { createHash } from "node:crypto";
import type { LocalDaemon, PreferenceKey, PrivilegedAction, SetupResult, SetupStep } from "@aether-ui/protocol";
import { revokedRule, type DaemonKind } from "../security/classes.ts";
import { daemonSlot, valueSlot, type Confirmation } from "../security/confirm.ts";
import { normalizeOrigin } from "../security/origins.ts";
import { BridgeConnectionLost, BridgeRefusal, Validator, type Sender, type ValidCall } from "../security/validate.ts";
import { ConnectionTable, type LocalDaemonRecord } from "./connections.ts";
import { okData, type DaemonLink, type Frame } from "./daemon-link.ts";
import { planAction, type Read } from "./privileged.ts";

/** What the bridge needs from Electron and the machine; tests hand in fakes. */
export interface BridgeHost {
  /** Every local daemon kind with its socket and health; the socket never leaves main. */
  findDaemons(): Promise<Array<LocalDaemonRecord & { reachable: boolean; version?: string }>>;
  openLink(socket: string): Promise<DaemonLink>;
  push(owner: number, conn: string, frame: Frame): void;
  confirm(owner: number, confirmation: Confirmation): Promise<boolean>;
  openTerminal(argv: readonly string[]): Promise<void>;
  notify(note: { title: string; body: string; route?: string }): void;
  /** Managed start: refused unless desktop.managed_start is on. */
  startDaemon(kind: DaemonKind): Promise<void>;
  /** The native folder picker; an opaque single-use grant for the choice, or null. */
  pickDirectory(owner: number, purpose: string): Promise<string | null>;
  openExternal(url: string): Promise<void>;
  openRemote(origin: string): Promise<void>;
  getPreference(key: PreferenceKey): unknown;
  setPreference(key: PreferenceKey, value: unknown): Promise<void>;
  /** One step of setting up this computer; its daemons come back without socket paths. */
  setup(owner: number, step: SetupStep): Promise<SetupResult>;
  readonly versions: Readonly<Record<string, string>>;
  readonly logPath: string;
  readonly protocolVersions: Readonly<Record<"aether" | "archon", number>>;
}

// On main's own control connection: the hello is 1, the confirmed frame 2, reads after it.
const HELLO_REQ = 1;
const ACTION_REQ = 2;
const FIRST_READ_REQ = 3;

/** A bridge refusal is known to precede delivery; return it as the request's error reply. */
export async function requestWithRefusal(handle: () => Promise<unknown>, frame: unknown): Promise<unknown> {
  try {
    return await handle();
  } catch (error) {
    const req = frame !== null && typeof frame === "object" ? (frame as Frame).req : undefined;
    if (!(error instanceof BridgeRefusal) || error instanceof BridgeConnectionLost || !Number.isSafeInteger(req) || (req as number) < 1) throw error;
    return { frame: "reply", id: req, result: { Err: { code: "forbidden", message: error.message } } };
  }
}

export class Bridge {
  readonly connections: ConnectionTable;
  private readonly validator: Validator;
  private readonly host: BridgeHost;

  constructor(validator: Validator, host: BridgeHost) {
    this.validator = validator;
    this.host = host;
    this.connections = new ConnectionTable({ openLink: (socket) => host.openLink(socket), push: (...args) => host.push(...args) });
  }

  /** The one entry of every IPC call: validate, then serve. `owner` is the sender's webContents id. */
  async handle(sender: Sender, owner: number, method: string, args: readonly unknown[]): Promise<unknown> {
    const call = this.validator.check(sender, method, args, this.connections);
    return this.serve(owner, call);
  }

  private async serve(owner: number, call: ValidCall): Promise<unknown> {
    switch (call.method) {
      case "discover": return (await this.host.findDaemons()).map(publicDaemon);
      case "connect": return this.connections.connect(owner, await this.daemon(call.daemon));
      case "request":
        await this.refuseGlobalRuleRevocation(owner, call.conn, call.frame);
        await this.confirmMemoryAccess(owner, call.conn, call.frame);
        return this.connections.request(owner, call.conn, call.frame, call.text);
      case "subscribe":
        await this.refuseGlobalRuleRevocation(owner, call.conn, call.frame);
        await this.confirmMemoryAccess(owner, call.conn, call.frame);
        return this.connections.subscribe(owner, call.conn, call.frame, call.text);
      case "close": return this.connections.close(owner, call.conn);
      case "privileged": return this.privileged(owner, call.action);
      case "startDaemon": return this.host.startDaemon(call.kind);
      case "pickDirectory": return this.host.pickDirectory(owner, call.purpose);
      case "notify": return this.host.notify(call.note);
      case "openExternal": return this.host.openExternal(externalUrl(call.url));
      case "openRemote": return this.openRemote(call.origin);
      case "diagnostics": return this.diagnostics();
      case "getPreference": return this.host.getPreference(call.key);
      case "setPreference": return this.host.setPreference(call.key, call.value);
      case "setup": return this.host.setup(owner, call.step);
    }
  }

  private async confirmMemoryAccess(owner: number, conn: string, frame: Frame): Promise<void> {
    const exporting = frame.cmd === "memory_export";
    if (!exporting && (frame.dry_run !== false || (frame.cmd !== "consolidate_memories" && frame.cmd !== "memory_import"))) return;
    const daemon = daemonSlot(this.connections.daemonOf(owner, conn));
    const importing = frame.cmd === "memory_import";
    if (importing && typeof frame.artifact !== "string") throw new BridgeRefusal("Import needs an artifact");
    const scope = JSON.stringify(frame.scope ?? "all");
    if (scope.length > 1024) throw new BridgeRefusal("The memory scope is too long to confirm");
    const target = importing ? `SHA-256 ${createHash("sha256").update(frame.artifact as string).digest("hex")}`
      : `Scope ${valueSlot(scope)}`;
    const action = importing ? "Import memories" : exporting ? "Export memories" : "Consolidate memories";
    if (!(await this.host.confirm(owner, { title: action, message: `${action}?`, detail: `${target}\nDaemon ${daemon}`, confirm: action }))) {
      throw new BridgeRefusal("Cancelled");
    }
  }

  private async daemon(id: string): Promise<LocalDaemonRecord> {
    const found = (await this.host.findDaemons()).find((daemon) => daemon.id === id);
    if (found === undefined) throw new BridgeRefusal(`No local daemon ${id}`);
    return found;
  }

  /** Builds, confirms and sends one privileged action on a control connection of main's own. */
  private async privileged(owner: number, action: PrivilegedAction): Promise<unknown> {
    const daemon = this.connections.daemonOf(owner, action.conn);
    return this.onControlLink(daemon, async (link, read) => {
      const plan = await planAction(action, daemon, ACTION_REQ, read);
      if (!(await this.host.confirm(owner, plan.confirmation))) throw new BridgeRefusal("Cancelled");
      if (plan.delivery.kind === "terminal") return await this.host.openTerminal(plan.delivery.argv);
      return okData(await link.request(plan.delivery.text, plan.delivery.req));
    });
  }

  /** A rule for every agent is the host's policy, as aetherd holds it for a device (`authz.rs`). */
  private async refuseGlobalRuleRevocation(owner: number, conn: string, frame: Frame): Promise<void> {
    const kind = this.connections.kindOf(conn);
    if (kind === undefined) throw new BridgeRefusal(`${conn} is not an open local connection`);
    const rule = revokedRule(kind, frame);
    if (rule === undefined) return;
    const daemon = this.connections.daemonOf(owner, conn);
    const rules = await this.onControlLink(daemon, (_link, read) => read({ cmd: "list_approval_rules", include_revoked: true }));
    if (!Array.isArray(rules)) throw new Error("list_approval_rules answers a list");
    const found = rules.find((entry) => (entry as { id?: unknown }).id === rule) as { scope?: { scope?: unknown } } | undefined;
    // aetherd decodes non-canonical spellings (rule_8… is rule_0…), so only an id it listed may pass.
    if (found === undefined) throw new BridgeRefusal(`${rule} is not one of this host's rules as listed`);
    if (typeof found.scope?.scope !== "string") throw new Error("list_approval_rules gives each rule a scope");
    if (found.scope.scope === "everywhere") {
      throw new BridgeRefusal(`${rule} applies to every agent; revoke it at the host`);
    }
  }

  /** Runs `work` on a control connection of main's own, greeted by main and closed after. */
  private async onControlLink<T>(daemon: LocalDaemonRecord, work: (link: DaemonLink, read: Read) => Promise<T>): Promise<T> {
    const link = await this.host.openLink(daemon.socket);
    try {
      okData(await link.request(JSON.stringify(this.hello(daemon.kind)), HELLO_REQ));
      let next = FIRST_READ_REQ;
      const read = async (body: Frame) => {
        const req = next++;
        return okData(await link.request(JSON.stringify({ ...body, req }), req));
      };
      return await work(link, read);
    } finally {
      link.close();
    }
  }

  private hello(kind: DaemonKind): Frame {
    return helloFrame(kind, this.host.protocolVersions[kind], this.host.versions.app ?? "0.0.0");
  }

  private async openRemote(value: string): Promise<void> {
    const origin = normalizeOrigin(value);
    const allowed = this.host.getPreference("remote.origins");
    if (!Array.isArray(allowed) || !allowed.includes(origin)) throw new BridgeRefusal(`${origin} is not in remote.origins`);
    await this.host.openRemote(origin);
  }

  private async diagnostics(): Promise<{ versions: Record<string, string>; daemons: LocalDaemon[]; logPath: string }> {
    const daemons = (await this.host.findDaemons()).map(publicDaemon);
    return { versions: { ...this.host.versions }, daemons, logPath: this.host.logPath };
  }
}

/** Main's hello to a local daemon, request 1: on a control link, and for a health probe. */
export function helloFrame(kind: DaemonKind, protocol_version: number, client_version: string): Frame {
  if (!Number.isSafeInteger(protocol_version) || protocol_version < 1) throw new Error("a protocol version is a positive integer");
  if (kind === "archon") return { cmd: "hello", req: HELLO_REQ, protocol_version, client: "archon-cli", client_version };
  return { cmd: "hello", req: HELLO_REQ, protocol_version, client_name: "aether-desktop", client_version };
}

/** What the renderer learns of a daemon: its health, never its socket path. */
export function publicDaemon(daemon: LocalDaemonRecord & { reachable: boolean; version?: string }): LocalDaemon {
  const found: LocalDaemon = { id: daemon.id, kind: daemon.kind, label: daemon.label, reachable: daemon.reachable };
  if (daemon.version !== undefined) found.version = daemon.version;
  return found;
}

/** An https: URL with no user name or password, as given; anything else is refused. */
export function externalUrl(value: string): string {
  const url = URL.parse(value);
  if (url === null) throw new BridgeRefusal("Not a URL");
  if (url.protocol !== "https:") throw new BridgeRefusal(`openExternal opens https: links only, not ${url.protocol}`);
  if (url.username !== "" || url.password !== "") throw new BridgeRefusal("openExternal refuses links with a user name or password");
  return url.href;
}
