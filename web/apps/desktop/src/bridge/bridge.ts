// The bridge's main side: every IPC call enters through `handle`, passes the Validator, and
// only then reaches the method that serves it. Electron-specific effects come in through BridgeHost.

import type { LocalDaemon, PreferenceKey, PrivilegedAction } from "@aether-ui/protocol";
import type { Confirmation } from "../security/confirm.ts";
import { normalizeOrigin } from "../security/origins.ts";
import { BridgeRefusal, Validator, type Sender, type ValidCall } from "../security/validate.ts";
import { ConnectionTable, type LocalDaemonRecord } from "./connections.ts";
import { okData, type DaemonLink, type Frame } from "./daemon-link.ts";
import { planAction } from "./privileged.ts";

/** What the bridge needs from Electron and the machine; tests hand in fakes. */
export interface BridgeHost {
  findDaemons(): Promise<Array<LocalDaemonRecord & { reachable: boolean }>>;
  openLink(socket: string): Promise<DaemonLink>;
  push(owner: number, conn: string, frame: Frame): void;
  confirm(owner: number, confirmation: Confirmation): Promise<boolean>;
  openTerminal(argv: readonly string[]): Promise<void>;
  notify(note: { title: string; body: string }): void;
  openExternal(url: string): Promise<void>;
  openRemote(origin: string): Promise<void>;
  getPreference(key: PreferenceKey): unknown;
  setPreference(key: PreferenceKey, value: unknown): Promise<void>;
  readonly versions: Readonly<Record<string, string>>;
  readonly logPath: string;
  readonly protocolVersions: Readonly<Record<"aether" | "archon", number>>;
}

// On main's own control connection: the hello is 1, the confirmed frame 2, reads after it.
const HELLO_REQ = 1;
const ACTION_REQ = 2;
const FIRST_READ_REQ = 3;

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
      case "request": return this.connections.request(owner, call.conn, call.frame, call.text);
      case "subscribe": return this.connections.subscribe(owner, call.conn, call.frame, call.text);
      case "close": return this.connections.close(owner, call.conn);
      case "privileged": return this.privileged(owner, call.action);
      case "startDaemon": return this.startDaemon();
      case "pickDirectory": throw new BridgeRefusal("Choosing a folder is not available in this build yet");
      case "notify": return this.host.notify({ title: call.note.title, body: call.note.body });
      case "openExternal": return this.host.openExternal(externalUrl(call.url));
      case "openRemote": return this.openRemote(call.origin);
      case "diagnostics": return this.diagnostics();
      case "getPreference": return this.host.getPreference(call.key);
      case "setPreference": return this.host.setPreference(call.key, call.value);
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
    const link = await this.host.openLink(daemon.socket);
    try {
      okData(await link.request(JSON.stringify(this.hello(daemon.kind)), HELLO_REQ));
      let next = FIRST_READ_REQ;
      const read = async (body: Frame) => {
        const req = next++;
        return okData(await link.request(JSON.stringify({ ...body, req }), req));
      };
      const plan = await planAction(action, daemon, ACTION_REQ, read);
      if (!(await this.host.confirm(owner, plan.confirmation))) throw new BridgeRefusal("Cancelled");
      if (plan.delivery.kind === "terminal") return await this.host.openTerminal(plan.delivery.argv);
      return okData(await link.request(plan.delivery.text, plan.delivery.req));
    } finally {
      link.close();
    }
  }

  private hello(kind: "aether" | "archon"): Frame {
    const protocol_version = this.host.protocolVersions[kind];
    const client_version = this.host.versions.app ?? "0.0.0";
    if (kind === "archon") return { cmd: "hello", req: HELLO_REQ, protocol_version, client: "archon-cli", client_version };
    return { cmd: "hello", req: HELLO_REQ, protocol_version, client_name: "aether-desktop", client_version };
  }

  private startDaemon(): never {
    if (this.host.getPreference("desktop.managed_start") !== true) {
      throw new BridgeRefusal("Starting daemons from the app is off; turn on desktop.managed_start first");
    }
    throw new BridgeRefusal("Starting daemons from the app is not available in this build yet");
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

/** What the renderer learns of a daemon: never its socket path. */
function publicDaemon(daemon: LocalDaemonRecord & { reachable: boolean }): LocalDaemon {
  return { id: daemon.id, kind: daemon.kind, label: daemon.label, reachable: daemon.reachable };
}

/** An https: URL with no user name or password, as given; anything else is refused. */
export function externalUrl(value: string): string {
  const url = URL.parse(value);
  if (url === null) throw new BridgeRefusal("Not a URL");
  if (url.protocol !== "https:") throw new BridgeRefusal(`openExternal opens https: links only, not ${url.protocol}`);
  if (url.username !== "" || url.password !== "") throw new BridgeRefusal("openExternal refuses links with a user name or password");
  return url.href;
}
