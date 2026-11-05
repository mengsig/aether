// The hermetic guard's evidence, read from /proc while the suite runs: every inet socket a
// harness process holds must be loopback, and no daemon may hold a file outside its root.

import assert from "node:assert/strict";
import { readdir, readFile, readlink, stat } from "node:fs/promises";
import path from "node:path";
import { userHome } from "./env.ts";

const SAMPLE_MS = 100;
const TABLES = ["tcp", "tcp6", "udp", "udp6", "raw", "raw6"] as const;
/** Names a credential would travel under; none may reach a daemon. */
const CREDENTIAL = /TOKEN|SECRET|PASSWORD|API_KEY|ACCESS_KEY|CREDENTIAL|AUTH/i;
/** Read-only system locations a daemon may hold open. */
const SYSTEM = ["/dev/", "/proc/", "/sys/", "/usr/", "/etc/", "/run/systemd/", "/var/lib/", "anon_inode:", "pipe:", "/memfd:"];

export interface Socket { readonly table: string; readonly local: string; readonly remote: string; readonly inode: string }

/** `0100007F:1F90` (little-endian IPv4) or 32 hex digits of IPv6, as /proc/net prints them. */
export function decodeAddress(hex: string): string {
  const [host = "", port = ""] = hex.toLowerCase().split(":");
  const portNumber = Number.parseInt(port, 16);
  if (host.length === 8) {
    const bytes = host.match(/../g)?.map((byte) => Number.parseInt(byte, 16)).reverse() ?? [];
    return `${bytes.join(".")}:${portNumber}`;
  }
  assert.equal(host.length, 32, `an IPv6 address in /proc/net is 32 hex digits: ${hex}`);
  const words = host.match(/.{8}/g)?.map((word) => word.match(/../g)?.reverse().join("") ?? "") ?? [];
  const groups = words.join("").match(/.{4}/g)?.map((group) => group.replace(/^0+(?=.)/, "")) ?? [];
  return `[${groups.join(":")}]:${portNumber}`;
}

/** 127.0.0.0/8, ::1 and IPv4-mapped loopback. A wildcard (0.0.0.0) is not loopback. */
export function isLoopback(address: string): boolean {
  const host = address.slice(0, address.lastIndexOf(":"));
  if (/^127\.\d+\.\d+\.\d+$/.test(host)) return true;
  if (host === "[0:0:0:0:0:0:0:1]") return true;
  return /^\[0:0:0:0:0:ffff:7f[0-9a-f]{2}:[0-9a-f]{1,4}\]$/.test(host);
}

/** An unconnected socket's remote end is all zeros: no peer, so nothing to check there. */
function hasPeer(address: string): boolean {
  return !/^(0\.0\.0\.0|\[0:0:0:0:0:0:0:0\]):0$/.test(address);
}

async function socketTable(pid: number): Promise<Map<string, Socket>> {
  const sockets = new Map<string, Socket>();
  for (const table of TABLES) {
    const text = await readFile(`/proc/${pid}/net/${table}`, "utf8").catch(() => "");
    for (const line of text.split("\n").slice(1)) {
      const fields = line.trim().split(/\s+/);
      if (fields.length < 10) continue;
      const [, local = "", remote = ""] = fields;
      const inode = fields[9] ?? "";
      sockets.set(inode, { table, local: decodeAddress(local), remote: decodeAddress(remote), inode });
    }
  }
  return sockets;
}

/** `roots` and every descendant still alive, from one pass over /proc. */
export async function processTree(roots: readonly number[]): Promise<number[]> {
  const parents = new Map<number, number[]>();
  for (const entry of await readdir("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    const stat = await readFile(`/proc/${entry}/stat`, "utf8").catch(() => null);
    if (stat === null) continue;
    const ppid = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
    parents.set(ppid, [...(parents.get(ppid) ?? []), Number(entry)]);
  }
  const seen = new Set<number>();
  const queue = [...roots];
  while (queue.length > 0) {
    const pid = queue.pop() as number;
    if (seen.has(pid)) continue;
    seen.add(pid);
    queue.push(...(parents.get(pid) ?? []));
  }
  return [...seen];
}

async function openTargets(pid: number): Promise<string[]> {
  const fds = await readdir(`/proc/${pid}/fd`).catch(() => [] as string[]);
  const targets = await Promise.all(fds.map((fd) => readlink(`/proc/${pid}/fd/${fd}`).catch(() => null)));
  return targets.filter((target): target is string => target !== null);
}

export interface Audit {
  readonly violations: readonly string[];
  readonly samples: number;
  readonly socketsSeen: number;
  stop(): void;
}

export interface AuditTarget {
  readonly root: () => string;
  /** The binaries under test: a daemon may read its own executable, which lies outside the root. */
  readonly binaries: readonly string[];
  readonly daemons: () => number[];
  readonly clients: () => number[];
}

function outsideRoot(open: string, root: string, binaries: readonly string[]): boolean {
  if (!path.isAbsolute(open) || open.startsWith(`${root}/`) || binaries.includes(open)) return false;
  return !SYSTEM.some((prefix) => open.startsWith(prefix) || `${open}/` === prefix);
}

/**
 * Samples every 100 ms until stopped. `daemons` are checked for sockets and files (inside
 * `root` only); `clients` (the browser) for sockets only.
 */
export function startAudit(target: AuditTarget): Audit {
  assert.ok(target.binaries.length > 0 && target.binaries.every((binary) => path.isAbsolute(binary)), "the binaries are absolute paths");
  const violations = new Set<string>();
  const sockets = new Set<string>();
  let samples = 0;
  let running = true;
  const sample = async () => {
    const daemons = await processTree(target.daemons());
    const clients = await processTree(target.clients());
    const root = target.root();
    for (const pid of [...daemons, ...clients]) {
      const table = await socketTable(pid);
      for (const open of await openTargets(pid)) {
        const inode = /^socket:\[(\d+)\]$/.exec(open)?.[1];
        const socket = inode === undefined ? undefined : table.get(inode);
        if (socket !== undefined) {
          sockets.add(socket.inode);
          if (!isLoopback(socket.local) || (hasPeer(socket.remote) && !isLoopback(socket.remote))) {
            violations.add(`pid ${pid}: ${socket.table} ${socket.local} -> ${socket.remote}`);
          }
        } else if (inode === undefined && daemons.includes(pid) && outsideRoot(open, root, target.binaries)) {
          violations.add(`pid ${pid} holds ${open}, outside the stack root`);
        }
      }
    }
    samples += 1;
  };
  const loop = async () => {
    while (running) {
      await sample().catch((error: unknown) => violations.add(`the audit could not sample: ${String(error)}`));
      await new Promise((resolve) => setTimeout(resolve, SAMPLE_MS));
    }
  };
  void loop();
  return {
    get violations() { return [...violations]; },
    get samples() { return samples; },
    get socketsSeen() { return sockets.size; },
    stop() { running = false; },
  };
}

/** What a daemon process was started with: every directory variable inside `root`, no credential. */
export async function checkEnvironment(pid: number, root: string): Promise<string[]> {
  const raw = await readFile(`/proc/${pid}/environ`, "utf8");
  const env = new Map(raw.split("\0").filter(Boolean).map((pair) => [pair.slice(0, pair.indexOf("=")), pair.slice(pair.indexOf("=") + 1)]));
  assert.ok(env.has("HOME") && env.has("PATH"), `pid ${pid} has an environment to check`);
  const problems: string[] = [];
  for (const [name, value] of env) {
    if (CREDENTIAL.test(name)) problems.push(`pid ${pid} has ${name} set`);
    const directory = name === "HOME" || name === "TMPDIR" || /^(XDG|AETHER|ARCHON)_.*(HOME|DIR|SOCKET)$/.test(name);
    if (directory && !value.startsWith(`${root}/`)) problems.push(`pid ${pid}: ${name}=${value} is outside ${root}`);
  }
  for (const name of ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_RUNTIME_DIR", "AETHER_CONFIG_DIR", "AETHER_DATA_DIR", "ARCHON_CONFIG_DIR", "ARCHON_DATA_DIR"]) {
    if (!env.has(name)) problems.push(`pid ${pid} has no ${name}, so it would fall back to the user's own`);
  }
  return problems;
}

/** The user's own Aether and Archon config dirs, as path → size and mtime; "absent" when missing. */
export async function snapshotUserConfig(home = userHome()): Promise<Map<string, string>> {
  const snapshot = new Map<string, string>();
  for (const directory of [path.join(home, ".config", "aether"), path.join(home, ".config", "archon")]) {
    const entries = await readdir(directory, { recursive: true }).catch(() => null);
    if (entries === null) {
      snapshot.set(directory, "absent");
      continue;
    }
    for (const entry of [".", ...entries]) {
      const file = path.join(directory, entry);
      const info = await stat(file).catch(() => null);
      snapshot.set(file, info === null ? "vanished" : `${info.size}@${info.mtimeMs}`);
    }
  }
  return snapshot;
}

export function diffSnapshots(before: ReadonlyMap<string, string>, after: ReadonlyMap<string, string>): string[] {
  const changes: string[] = [];
  for (const [file, was] of before) if (after.get(file) !== was) changes.push(`${file}: ${was} -> ${after.get(file) ?? "gone"}`);
  for (const file of after.keys()) if (!before.has(file)) changes.push(`${file}: new`);
  return changes;
}
