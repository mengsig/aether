// The local daemons the app can reach: each kind's socket by its CLI's rules, probed for health.
// Socket paths stay in these records, which never cross IPC; the bridge strips them.

import type { LocalDaemonRecord } from "../bridge/connections.ts";
import type { Frame } from "../bridge/daemon-link.ts";
import type { DaemonKind } from "../security/classes.ts";
import { probe, PROBE_TIMEOUT_MS, type Health } from "./health.ts";
import { socketPath, type Machine } from "./socket-paths.ts";

export type FoundDaemon = LocalDaemonRecord & Health;

export const DAEMON_KINDS: readonly DaemonKind[] = ["aether", "archon"];
const LABELS: Readonly<Record<DaemonKind, string>> = { aether: "aetherd", archon: "archond" };

/** The record of each daemon kind on `machine`, unprobed. */
export function localDaemons(machine: Machine): LocalDaemonRecord[] {
  return DAEMON_KINDS.map((kind) => ({ id: `${kind}:local`, kind, label: LABELS[kind], socket: socketPath(kind, machine) }));
}

/** Every daemon kind with its health, probed side by side. */
export async function findDaemons(machine: Machine, hello: (kind: DaemonKind) => Frame): Promise<FoundDaemon[]> {
  const records = localDaemons(machine);
  const health = await Promise.all(records.map((record) =>
    probe(record.socket, { hello: hello(record.kind), timeoutMs: PROBE_TIMEOUT_MS })));
  if (health.length !== records.length) throw new Error("one health answer per daemon");
  return records.map((record, index) => ({ ...record, ...health[index] }) as FoundDaemon);
}
