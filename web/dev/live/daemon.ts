// A disposable aetherd for the live checks: its own config, data and socket under /var/tmp, the
// remote listener on a free loopback port, and host commands over the local socket.

import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { closeSync, openSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { createInterface } from "node:readline";

export type Reply = { readonly reply: string; readonly data: Record<string, unknown> };

export interface Daemon {
  /** `http://127.0.0.1:<port>`, the listener's loopback origin. */
  readonly url: string;
  readonly root: string;
  request(command: Record<string, unknown>): Promise<Reply>;
  close(): Promise<void>;
}

const PROTOCOL_VERSION = 26;
const WAIT_MS = 15_000;
const CALLER_ONLY = new Set(["AETHER_REMOTE", "AETHER_ADDRESS", "AETHER_TOKEN"]);

export async function until<T>(read: () => Promise<T>, accepts: (value: T) => boolean, what: string, timeoutMs = WAIT_MS): Promise<T> {
  if (what === "" || timeoutMs <= 0) throw new Error("polling needs a description and a positive timeout");
  const end = Date.now() + timeoutMs;
  let value: T | undefined;
  while (Date.now() < end) {
    value = await read();
    if (accepts(value)) return value;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for ${what}: ${JSON.stringify(value)}`);
}

async function freePort(): Promise<number> {
  const server = net.createServer();
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (address === null || typeof address === "string") throw new Error("could not reserve a loopback port");
  await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
  return address.port;
}

/** One host command over the daemon's Unix socket: hello, then the command, then its reply. */
export async function hostRequest(socketPath: string, command: Record<string, unknown>): Promise<Reply> {
  if (!socketPath.startsWith("/var/tmp/")) throw new Error(`the live daemon's socket is under /var/tmp, not ${socketPath}`);
  if (typeof command.cmd !== "string") throw new Error("a host command names its cmd");
  const socket = net.createConnection(socketPath);
  const lines = createInterface({ input: socket });
  const timer = setTimeout(() => socket.destroy(new Error(`host request timed out: ${String(command.cmd)}`)), WAIT_MS);
  try {
    await new Promise((resolve, reject) => { socket.once("connect", resolve); socket.once("error", reject); });
    socket.write(`${JSON.stringify({ req: 1, cmd: "hello", client_name: "aether-web-live", client_version: "0.0.0", protocol_version: PROTOCOL_VERSION, features: [] })}\n`);
    for await (const line of lines) {
      const frame = JSON.parse(line) as { frame?: string; id?: number; result?: { Ok?: Reply; Err?: unknown } };
      if (frame.frame !== "reply") continue;
      if (frame.result?.Err !== undefined) throw new Error(`${String(command.cmd)}: ${JSON.stringify(frame.result.Err)}`);
      if (frame.id === 1) socket.write(`${JSON.stringify({ req: 2, ...command })}\n`);
      if (frame.id === 2 && frame.result?.Ok) return frame.result.Ok;
    }
    throw new Error(`the daemon closed before answering ${String(command.cmd)}`);
  } finally {
    clearTimeout(timer);
    lines.close();
    socket.destroy();
  }
}

async function stop(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const ended = new Promise((resolve) => child.once("exit", resolve));
  child.kill("SIGTERM");
  const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
  await ended;
  clearTimeout(timer);
}

/** Starts `binary` as a foreground daemon with remote access on, and waits until it answers. */
export async function startDaemon(binary: string): Promise<Daemon> {
  if (!path.isAbsolute(binary)) throw new Error(`the daemon binary path is absolute, not ${binary}`);
  const root = await mkdtemp("/var/tmp/aw-");
  const socket = path.join(root, "daemon.sock");
  // The caller's own daemon address or token must not leak into the disposable one.
  const inherited = Object.entries(process.env).filter(([name]) => !CALLER_ONLY.has(name));
  const env: NodeJS.ProcessEnv = { ...Object.fromEntries(inherited), AETHER_CONFIG_DIR: path.join(root, "config"), AETHER_DATA_DIR: path.join(root, "data"), AETHER_SOCKET: socket };
  let daemon: ChildProcess | undefined;
  try {
    const port = await freePort();
    for (const directory of ["config", "data", "work"]) await mkdir(path.join(root, directory));
    await writeFile(path.join(root, "config", "config.toml"), `[remote]\naddress = "127.0.0.1:${port}"\n`);
    const log = openSync(path.join(root, "daemon.log"), "w");
    daemon = spawn(binary, ["--local", "daemon", "--foreground"], { cwd: path.join(root, "work"), env, stdio: ["ignore", log, log] });
    closeSync(log);
    await until(() => readFile(path.join(root, "daemon.log"), "utf8"), (text) => text.includes("listening"), "the daemon socket");
    execFileSync(binary, ["--local", "remote", "enable"], { env, cwd: path.join(root, "work"), stdio: "pipe" });
    const running = daemon;
    return {
      url: `http://127.0.0.1:${port}`,
      root,
      request: (command) => hostRequest(socket, command),
      close: async () => { await stop(running); await rm(root, { recursive: true, force: true }); },
    };
  } catch (error) {
    const log = await readFile(path.join(root, "daemon.log"), "utf8").catch(() => "(no daemon log)");
    if (daemon) await stop(daemon);
    await rm(root, { recursive: true, force: true });
    throw new Error(`Could not start the live daemon. Its log:\n${log}`, { cause: error });
  }
}
