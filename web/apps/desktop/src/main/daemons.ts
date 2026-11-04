// The local daemons main can reach, found the way the CLIs find their sockets: the environment
// variable first, then the runtime directory, then the data directory. E2 adds health and status.

import { statSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import type { LocalDaemonRecord } from "../bridge/connections.ts";

type Env = Readonly<Record<string, string | undefined>>;

function dataDir(env: Env, name: string, override: string): string {
  const explicit = env[override];
  if (explicit) return explicit;
  if (process.platform === "darwin") return path.join(os.homedir(), "Library", "Application Support", name);
  return path.join(env.XDG_DATA_HOME || path.join(os.homedir(), ".local", "share"), name);
}

/** The socket path each daemon's own CLI would use. */
export function socketPaths(env: Env): Record<"aether" | "archon", string> {
  const runtime = env.XDG_RUNTIME_DIR;
  const pick = (socketVar: string, name: string, file: string, dataVar: string) =>
    env[socketVar] || (runtime ? path.join(runtime, name, file) : path.join(dataDir(env, name, dataVar), file));
  return {
    aether: pick("AETHER_SOCKET", "aether", "aetherd.sock", "AETHER_DATA_DIR"),
    archon: pick("ARCHON_SOCKET", "archon", "archond.sock", "ARCHON_DATA_DIR"),
  };
}

function isSocket(file: string): boolean {
  try {
    return statSync(file).isSocket();
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw new Error(`Checking the daemon socket ${file}`, { cause: error });
  }
}

export function findDaemons(env: Env): Array<LocalDaemonRecord & { reachable: boolean }> {
  const sockets = socketPaths(env);
  return (["aether", "archon"] as const).map((kind) => ({
    id: `${kind}:local`,
    kind,
    label: kind === "aether" ? "aetherd" : "archond",
    socket: sockets[kind],
    reachable: isSocket(sockets[kind]),
  }));
}
