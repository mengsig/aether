// The local connections main holds for the app window. Each id is main's own and bound to the
// webContents that opened it; nothing else can reach its socket, and a remote window never can.

import { randomUUID } from "node:crypto";
import type { DaemonKind } from "../security/classes.ts";
import type { DaemonIdentity } from "../security/confirm.ts";
import { BridgeRefusal, type Connections } from "../security/validate.ts";
import type { DaemonLink, Frame } from "./daemon-link.ts";

/** A daemon main found on this machine, with the socket the renderer never sees. */
export interface LocalDaemonRecord extends DaemonIdentity {
  readonly id: string;
}

export interface ConnectionDeps {
  openLink(socket: string): Promise<DaemonLink>;
  /** Hands a frame that answers no `request` to the window that owns the connection. */
  push(owner: number, conn: string, frame: Frame): void;
}

interface Entry {
  readonly owner: number;
  readonly daemon: LocalDaemonRecord;
  readonly link: DaemonLink;
  /** Taken from the daemon's welcome only, never from a frame the renderer sent. */
  protocol?: number;
}

export class ConnectionTable implements Connections {
  private readonly entries = new Map<string, Entry>();
  private readonly deps: ConnectionDeps;

  constructor(deps: ConnectionDeps) {
    this.deps = deps;
  }

  kindOf(conn: string): DaemonKind | undefined {
    return this.entries.get(conn)?.daemon.kind;
  }

  protocolOf(conn: string): number | undefined {
    return this.entries.get(conn)?.protocol;
  }

  async connect(owner: number, daemon: LocalDaemonRecord): Promise<string> {
    if (!Number.isInteger(owner)) throw new Error("a connection's owner is a webContents id");
    const link = await this.deps.openLink(daemon.socket);
    const conn = `local:${randomUUID()}`;
    if (this.entries.has(conn)) throw new Error("connection ids are unique");
    link.onFrame = (frame) => this.deps.push(owner, conn, frame);
    link.onClose = () => this.entries.delete(conn);
    this.entries.set(conn, { owner, daemon, link });
    return conn;
  }

  /** The daemon behind `conn`, for a privileged action's confirmation and control link. */
  daemonOf(owner: number, conn: string): LocalDaemonRecord {
    return this.entry(owner, conn).daemon;
  }

  async request(owner: number, conn: string, frame: Frame, text: string): Promise<Frame> {
    const entry = this.entry(owner, conn);
    const reply = await entry.link.request(text, frame.req as number);
    if (frame.cmd === "hello") entry.protocol = welcomeProtocol(reply);
    return reply;
  }

  /** Sends the frame; its reply and every frame after it arrive through `push`. */
  subscribe(owner: number, conn: string, frame: Frame, text: string): void {
    const { link } = this.entry(owner, conn);
    if (link.hasPending(frame.req as number)) throw new BridgeRefusal(`Request ${String(frame.req)} is already waiting`);
    link.send(text);
  }

  close(owner: number, conn: string): void {
    const entry = this.entries.get(conn);
    if (entry === undefined) return;
    if (entry.owner !== owner) throw new BridgeRefusal(`${conn} belongs to another window`);
    this.entries.delete(conn);
    entry.link.close();
  }

  /** Closes every connection of a window that went away. */
  closeOwner(owner: number): void {
    for (const [conn, entry] of [...this.entries]) {
      if (entry.owner !== owner) continue;
      this.entries.delete(conn);
      entry.link.close();
    }
  }

  private entry(owner: number, conn: string): Entry {
    const entry = this.entries.get(conn);
    if (entry === undefined) throw new BridgeRefusal(`${conn} is not an open local connection`);
    if (entry.owner !== owner) throw new BridgeRefusal(`${conn} belongs to another window`);
    if (!entry.link.isOpen) throw new BridgeRefusal(`${conn} is closed`);
    return entry;
  }
}

/** The protocol an `Ok` welcome reports, or undefined for a refusal or a malformed reply. */
function welcomeProtocol(reply: Frame): number | undefined {
  const data = (reply.result as { Ok?: { data?: { protocol_version?: unknown } | null } } | undefined)?.Ok?.data;
  const version = data?.protocol_version;
  return typeof version === "number" && Number.isSafeInteger(version) && version >= 1 ? version : undefined;
}
