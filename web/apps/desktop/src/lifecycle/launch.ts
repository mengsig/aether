// Runs a daemon's own start command fully detached from the app: its own session, stdin closed,
// stdout and stderr appended to a log file, and unref'd. The app keeps no hold on it, so quitting,
// closing windows or crashing stops nothing; nothing here can signal it either.

import { spawn, type ChildProcess } from "node:child_process";
import { closeSync, mkdirSync, openSync, writeSync } from "node:fs";
import path from "node:path";
import type { DaemonKind } from "../security/classes.ts";

export interface Launcher {
  readonly command: string;
  readonly args: readonly string[];
}

/** The commands the CLIs document for starting a background daemon; both return once it listens. */
export const LAUNCHERS: Readonly<Record<DaemonKind, Launcher>> = {
  aether: { command: "aether", args: ["daemon"] },
  archon: { command: "archon", args: ["daemon", "start"] },
};

export interface LaunchOptions {
  readonly env: Readonly<Record<string, string | undefined>>;
  /** The daemon's working directory, its default workspace: the user's home. */
  readonly cwd: string;
  readonly logFile: string;
}

/** The detached launcher process; the caller may watch it but can never stop it. */
export function launchDetached(launcher: Launcher, options: LaunchOptions): ChildProcess {
  if (!path.isAbsolute(options.logFile) || !path.isAbsolute(options.cwd)) throw new Error("launch paths are absolute");
  if (!/^[a-z]+$/.test(launcher.command)) throw new Error(`a launcher is a plain command name, not ${launcher.command}`);
  mkdirSync(path.dirname(options.logFile), { recursive: true, mode: 0o700 });
  const log = openSync(options.logFile, "a", 0o600);
  try {
    writeSync(log, `--- ${new Date().toISOString()} the desktop app runs: ${launcher.command} ${launcher.args.join(" ")}\n`);
    const child = spawn(launcher.command, [...launcher.args], {
      cwd: options.cwd,
      env: { ...options.env },
      detached: true,
      stdio: ["ignore", log, log],
      windowsHide: true,
    });
    child.unref();
    return child;
  } finally {
    // The child holds its own copy of the descriptor; the app keeps none.
    closeSync(log);
  }
}
