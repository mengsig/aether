// Optional managed start (ARCHITECTURE.md §c): off unless desktop.managed_start is true. When on,
// `startDaemon` runs the daemon's own start command detached and waits until the daemon answers.

import type { ChildProcess } from "node:child_process";
import path from "node:path";
import { absoluteHome, DAEMON_PATHS, join, type Env, type Machine } from "../discovery/socket-paths.ts";
import type { DaemonKind } from "../security/classes.ts";
import { BridgeRefusal } from "../security/validate.ts";
import { LAUNCHERS, launchDetached } from "./launch.ts";

/**
 * The launcher's environment: `env` with a relative HOME and each relative socket or data dir
 * override put under main's own directory, where discovery reads them, since the launcher runs
 * in home. Only prefixed, never normalized: `..` after a symlink must keep its meaning. An empty
 * data dir means that directory; an empty socket stays empty and reaches nothing.
 */
export function launchEnv(env: Env, mainCwd: string): Env {
  if (!mainCwd.startsWith("/")) throw new Error(`main's directory is absolute, not ${mainCwd}`);
  const moved: Record<string, string> = {};
  const put = (name: string, emptyIsHere: boolean) => {
    const value = env[name];
    if (value === undefined || value.startsWith("/") || (value === "" && !emptyIsHere)) return;
    moved[name] = value === "" ? mainCwd : join(mainCwd, value);
  };
  put("HOME", false);
  for (const rules of Object.values(DAEMON_PATHS)) {
    put(rules.socketVar, false);
    put(rules.dataVar, true);
  }
  return { ...env, ...moved };
}

/** Managed start on `machine`, launching in the user's home, from main running in `mainCwd`. */
export function managedStartOn(
  machine: Machine, mainCwd: string, deps: Pick<ManagedStartDeps, "isEnabled" | "isReachable" | "logDir">,
): ManagedStart {
  const env = launchEnv(machine.env, mainCwd);
  return new ManagedStart({ ...deps, env, cwd: absoluteHome({ ...machine, env }) });
}

export interface ManagedStartDeps {
  /** Whether the user turned on desktop.managed_start; it is off until they do. */
  isEnabled(): boolean;
  /** Whether the daemon answers on its socket now. */
  isReachable(kind: DaemonKind): Promise<boolean>;
  readonly env: Env;
  readonly cwd: string;
  readonly logDir: string;
  readonly deadlineMs?: number;
  readonly pollMs?: number;
}

const DEADLINE_MS = 30_000;
const POLL_MS = 200;
const LABELS: Readonly<Record<DaemonKind, string>> = { aether: "aetherd", archon: "archond" };

type LauncherState = { readonly ended: false } | { readonly ended: true; readonly failure?: string };

/** Follows the launcher without holding it: whether it ended, and how. */
function follow(child: ChildProcess, kind: DaemonKind, logFile: string): () => LauncherState {
  let state: LauncherState = { ended: false };
  const command = LAUNCHERS[kind].command;
  child.once("error", (error: NodeJS.ErrnoException) => {
    state = error.code === "ENOENT"
      ? { ended: true, failure: `${command} isn't installed, or isn't on the app's PATH.` }
      : { ended: true, failure: `${command} could not run (${error.code ?? error.message}).` };
  });
  child.once("exit", (code, signal) => {
    if (state.ended) return;
    state = code === 0 ? { ended: true } : { ended: true, failure: `${LABELS[kind]} didn't start (${signal ?? `exit ${code}`}). Its output is in ${logFile}.` };
  });
  return () => state;
}

export class ManagedStart {
  private readonly deps: ManagedStartDeps;
  private readonly starting = new Map<DaemonKind, Promise<void>>();

  constructor(deps: ManagedStartDeps) {
    if (!path.isAbsolute(deps.logDir) || !path.isAbsolute(deps.cwd)) throw new Error("managed start paths are absolute");
    this.deps = deps;
  }

  logFile(kind: DaemonKind): string {
    return path.join(this.deps.logDir, `managed-start-${kind}.log`);
  }

  /** Starts `kind` unless it already answers; one start per kind at a time. */
  start(kind: DaemonKind): Promise<void> {
    if (!(kind in LAUNCHERS)) throw new Error(`no launcher for ${String(kind)}`);
    if (this.deps.isEnabled() !== true) {
      return Promise.reject(new BridgeRefusal("Starting daemons from the app is off. Turn on desktop.managed_start to allow it."));
    }
    const running = this.starting.get(kind);
    if (running !== undefined) return running;
    const started = this.startOnce(kind).finally(() => this.starting.delete(kind));
    this.starting.set(kind, started);
    return started;
  }

  private async startOnce(kind: DaemonKind): Promise<void> {
    if (await this.deps.isReachable(kind)) return;
    const logFile = this.logFile(kind);
    const child = launchDetached(LAUNCHERS[kind], { env: this.deps.env, cwd: this.deps.cwd, logFile });
    const launcher = follow(child, kind, logFile);
    const end = Date.now() + (this.deps.deadlineMs ?? DEADLINE_MS);
    while (Date.now() < end) {
      await new Promise((resolve) => setTimeout(resolve, this.deps.pollMs ?? POLL_MS));
      const state = launcher();
      if (state.ended && state.failure !== undefined) throw new BridgeRefusal(state.failure);
      // The CLIs' start commands exit 0 once the daemon listens; a daemon may also answer first.
      if (state.ended || await this.deps.isReachable(kind)) return;
    }
    throw new BridgeRefusal(`${LABELS[kind]} is still starting. Check again in a moment.`);
  }
}
