// Child processes of the harness: each daemon runs in its own process group, so stopping it
// also stops what it spawned, and every spawn is recorded for scripts/e2e.sh's leak check.

import { spawn, type ChildProcess } from "node:child_process";
import { appendFileSync, closeSync, openSync, readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";

const STOP_GRACE_MS = 10_000;
const POLL_MS = 100;

export async function until<T>(read: () => Promise<T>, accepts: (value: T) => boolean, what: string, timeoutMs: number): Promise<T> {
  if (what === "" || timeoutMs <= 0) throw new Error("polling needs a description and a positive timeout");
  const end = Date.now() + timeoutMs;
  let value: T | undefined;
  while (Date.now() < end) {
    value = await read();
    if (accepts(value)) return value;
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  }
  throw new Error(`Timed out after ${timeoutMs} ms waiting for ${what}: ${JSON.stringify(value)?.slice(-2000)}`);
}

/** The kernel start time of `pid` (field 22 of /proc/pid/stat): with the pid, what no reused pid matches. */
export function startTime(pid: number): string | null {
  let stat: string;
  try {
    stat = readFileSync(`/proc/${pid}/stat`, "utf8");
  } catch {
    return null;
  }
  const fields = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
  const started = fields[19];
  if (started === undefined || !/^\d+$/.test(started)) throw new Error(`unreadable /proc/${pid}/stat: ${stat}`);
  return started;
}

/** Appends `line` to `$E2E_STATE/<file>` when scripts/e2e.sh asked for a record; a no-op otherwise. */
export function record(file: "pids" | "roots", line: string): void {
  const state = process.env.E2E_STATE;
  if (state === undefined || state === "") return;
  if (!path.isAbsolute(state)) throw new Error(`E2E_STATE is an absolute path, not ${state}`);
  if (line.includes("\n")) throw new Error(`a record is one line: ${JSON.stringify(line)}`);
  appendFileSync(path.join(state, file), `${line}\n`);
}

export interface Spawned {
  readonly child: ChildProcess;
  readonly pid: number;
  readonly log: string;
}

/** Starts `binary args` as a process-group leader with `env`, its output appended to `log`. */
export function spawnLogged(binary: string, args: readonly string[], options: { env: NodeJS.ProcessEnv; cwd: string; log: string }): Spawned {
  if (!path.isAbsolute(binary) || !path.isAbsolute(options.log)) throw new Error(`absolute paths only: ${binary}, ${options.log}`);
  const out = openSync(options.log, "a");
  let child: ChildProcess;
  try {
    child = spawn(binary, args, { cwd: options.cwd, env: options.env, stdio: ["ignore", out, out], detached: true });
  } finally {
    closeSync(out);
  }
  const pid = child.pid;
  if (pid === undefined) throw new Error(`${binary} did not start`);
  record("pids", `${pid} ${startTime(pid) ?? "gone"} ${path.basename(binary)}`);
  return { child, pid, log: options.log };
}

function signalGroup(pid: number, signal: NodeJS.Signals): void {
  try {
    process.kill(-pid, signal);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
  }
}

function groupAlive(pid: number): boolean {
  try {
    process.kill(-pid, 0);
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ESRCH") return false;
    throw error;
  }
}

/** SIGTERM to the whole group, SIGKILL after the grace; resolves once the group is empty. */
export async function stopGroup(spawned: Spawned): Promise<void> {
  const exited = spawned.child.exitCode !== null || spawned.child.signalCode !== null
    ? Promise.resolve()
    : new Promise<void>((resolve) => spawned.child.once("exit", () => resolve()));
  signalGroup(spawned.pid, "SIGTERM");
  const deadline = Date.now() + STOP_GRACE_MS;
  while (groupAlive(spawned.pid) && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  if (groupAlive(spawned.pid)) signalGroup(spawned.pid, "SIGKILL");
  await exited;
  await until(async () => groupAlive(spawned.pid), (alive) => !alive, `process group ${spawned.pid} to end`, STOP_GRACE_MS);
}

export async function tail(file: string, lines = 40): Promise<string> {
  const text = await readFile(file, "utf8").catch((error: unknown) => `(no log at ${file}: ${String(error)})`);
  return text.split("\n").slice(-lines).join("\n");
}
