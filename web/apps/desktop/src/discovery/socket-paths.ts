// Where each daemon's socket is, by the rules its own CLI uses: aether-protocol paths.rs
// `socket_path` and archon-core paths.rs `socket_path`, over the `dirs` crate's directories.

import type { DaemonKind } from "../security/classes.ts";

export type Env = Readonly<Record<string, string | undefined>>;

/** The platforms the desktop app claims; `dirs` gives each its own data and runtime directory. */
export type Platform = "linux" | "darwin";

export interface Machine {
  readonly env: Env;
  readonly platform: Platform;
  /** The account's home from the password database, used when $HOME is unset or empty. */
  readonly passwdHome: string;
}

interface DaemonPaths {
  readonly socketVar: string;
  readonly dataVar: string;
  readonly dir: string;
  readonly file: string;
}

export const DAEMON_PATHS: Readonly<Record<DaemonKind, DaemonPaths>> = {
  aether: { socketVar: "AETHER_SOCKET", dataVar: "AETHER_DATA_DIR", dir: "aether", file: "aetherd.sock" },
  archon: { socketVar: "ARCHON_SOCKET", dataVar: "ARCHON_DATA_DIR", dir: "archon", file: "archond.sock" },
};

/** Rust's `Path::join` on Unix: no normalizing, so `..` and doubled slashes stay as written. */
export function join(base: string, rest: string): string {
  if (base === "" || rest.startsWith("/")) return rest;
  return base.endsWith("/") ? `${base}${rest}` : `${base}/${rest}`;
}

/** `dirs::home_dir`: $HOME when it is set and not empty, else the password database. */
function homeDir(machine: Machine): string {
  const home = machine.env.HOME;
  return home !== undefined && home !== "" ? home : machine.passwdHome;
}

/** The home a process can start in: `dirs::home_dir` when absolute, else the password database's. */
export function absoluteHome(machine: Machine): string {
  const home = homeDir(machine);
  return home.startsWith("/") ? home : machine.passwdHome;
}

/** `dirs` reads an XDG variable only when it holds an absolute path. */
function absoluteVar(env: Env, name: string): string | undefined {
  const value = env[name];
  return value !== undefined && value.startsWith("/") ? value : undefined;
}

/** `dirs::runtime_dir`: $XDG_RUNTIME_DIR on Linux only. */
function runtimeDir(machine: Machine): string | undefined {
  return machine.platform === "linux" ? absoluteVar(machine.env, "XDG_RUNTIME_DIR") : undefined;
}

/** `dirs::data_dir`. */
function dataHome(machine: Machine): string {
  const home = homeDir(machine);
  if (machine.platform === "darwin") return join(home, "Library/Application Support");
  return absoluteVar(machine.env, "XDG_DATA_HOME") ?? join(home, ".local/share");
}

/**
 * The socket the `kind` CLI connects to with no --socket flag. A set variable wins even when
 * empty, as `std::env::var_os` reads it; an empty socket path then simply reaches nothing.
 */
export function socketPath(kind: DaemonKind, machine: Machine): string {
  const rules = DAEMON_PATHS[kind];
  if (rules === undefined) throw new Error(`no socket rules for ${String(kind)}`);
  const explicit = machine.env[rules.socketVar];
  if (explicit !== undefined) return explicit;
  const runtime = runtimeDir(machine);
  if (runtime !== undefined) return join(runtime, `${rules.dir}/${rules.file}`);
  const data = machine.env[rules.dataVar] ?? join(dataHome(machine), rules.dir);
  return join(data, rules.file);
}

/** This process's machine: `env` as given, the real platform and the real home. */
export function machineOf(env: Env, platform: NodeJS.Platform, passwdHome: string): Machine {
  if (platform !== "linux" && platform !== "darwin") {
    throw new Error(`the desktop app does not find daemons on ${platform}`);
  }
  if (!passwdHome.startsWith("/")) throw new Error(`the account's home is absolute, not ${passwdHome}`);
  return { env, platform, passwdHome };
}
