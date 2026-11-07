// The process test: Electron starts a stub daemon through managed start, then quits or crashes,
// and the daemon must still run and answer, untouched (no shutdown). The test stops it itself.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const HARNESS = fileURLToPath(new URL("./electron/harness.ts", import.meta.url));
const STUB = fileURLToPath(new URL("./stub-daemon.ts", import.meta.url));
const RESULT_PREFIX = "DESKTOP-LIFECYCLE-RESULTS ";
const RUN_TIMEOUT_MS = 90_000;
const EXPECTED = [
  "a link from before the window loaded navigates the page", "managed start is off by default", "managed start runs the daemon once the setting is on",
  "deep links reach the page and anything else is dropped",
];

interface Run { code: number | null; signal: NodeJS.Signals | null; stdout: string; stderr: string }

/** Runs the harness to its end; after a crash, helpers may hold its pipes a moment longer. */
function runHarness(dir: string, ending: "quit" | "crash"): Promise<Run> {
  const electron = createRequire(import.meta.url)("electron") as unknown as string;
  assert.equal(typeof electron, "string", "the electron package names its binary");
  const dropped = new Set(["DISPLAY", "WAYLAND_DISPLAY", "ELECTRON_RUN_AS_NODE", "AETHER_SOCKET", "ARCHON_SOCKET"]);
  const env: NodeJS.ProcessEnv = {
    ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !dropped.has(name))),
    DESKTOP_LIFECYCLE_DIR: dir, DESKTOP_LIFECYCLE_EXIT: ending,
  };
  const child = spawn(electron, ["--ozone-platform=headless", HARNESS], { env, stdio: ["ignore", "pipe", "pipe"] });
  const run: Run = { code: null, signal: null, stdout: "", stderr: "" };
  child.stdout.on("data", (chunk: Buffer) => { run.stdout += chunk.toString("utf8"); });
  child.stderr.on("data", (chunk: Buffer) => { run.stderr += chunk.toString("utf8"); });
  const timer = setTimeout(() => child.kill("SIGKILL"), RUN_TIMEOUT_MS);
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      clearTimeout(timer);
      Object.assign(run, { code, signal });
      const grace = setTimeout(() => resolve(run), 3_000);
      child.once("close", () => { clearTimeout(grace); resolve(run); });
    });
  });
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ESRCH") return false;
    throw error;
  }
}

/** The daemon answers hello on its socket, from outside Electron. */
function hello(socket: string): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const connection = net.createConnection({ path: socket });
    const timer = setTimeout(() => { connection.destroy(); reject(new Error("no answer within 5 s")); }, 5_000);
    let buffered = "";
    connection.once("error", reject);
    connection.on("connect", () => connection.write(`${JSON.stringify({ cmd: "hello", req: 1, protocol_version: 26 })}\n`));
    connection.on("data", (chunk) => {
      buffered += chunk.toString("utf8");
      if (!buffered.includes("\n")) return;
      clearTimeout(timer);
      connection.end();
      resolve(JSON.parse(buffered.slice(0, buffered.indexOf("\n"))) as Record<string, unknown>);
    });
  });
}

/** Stops the stub this test started, by its recorded pid, and waits for it to go. */
async function stopStub(pid: number): Promise<void> {
  if (!alive(pid)) return;
  process.kill(pid, "SIGTERM");
  for (let waited = 0; waited < 5_000 && alive(pid); waited += 50) await new Promise((done) => setTimeout(done, 50));
  if (alive(pid)) process.kill(pid, "SIGKILL");
}

async function survives(ending: "quit" | "crash"): Promise<void> {
  // Short, for the socket path limit; the stub's pid, commands and socket live here.
  const dir = mkdtempSync(path.join(os.tmpdir(), "e2-"));
  const pidFile = path.join(dir, "a.pid");
  mkdirSync(path.join(dir, "bin"));
  const aether = path.join(dir, "bin", "aether");
  writeFileSync(aether, `#!/bin/sh\nexec '${process.execPath}' '${STUB}' "$@"\n`);
  chmodSync(aether, 0o755);
  let pid: number | undefined;
  try {
    const run = await runHarness(dir, ending);
    if (existsSync(pidFile)) pid = Number(readFileSync(pidFile, "utf8").trim());
    const line = run.stdout.split("\n").find((text) => text.startsWith(RESULT_PREFIX));
    assert.ok(line !== undefined, `the harness reported no results (exit ${run.code}, ${run.signal}):\n${run.stderr.slice(-4000)}`);
    const results = JSON.parse(line.slice(RESULT_PREFIX.length)) as Array<{ name: string; ok: boolean; error?: string }>;
    const failed = results.filter((result) => !result.ok);
    assert.deepEqual(failed, [], failed.map((result) => `${result.name}: ${result.error}`).join("\n"));
    assert.deepEqual(results.map((result) => result.name), EXPECTED);
    if (ending === "quit") assert.equal(run.code, 0, "Electron quit cleanly");
    else assert.ok(run.signal !== null || (run.code !== 0 && run.code !== null), `Electron crashed (${run.code}, ${run.signal})`);

    assert.ok(pid !== undefined && Number.isSafeInteger(pid) && pid > 1, "the stub recorded its pid");
    assert.ok(alive(pid), `the daemon outlived Electron's ${ending}`);
    if (process.platform === "linux") {
      const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
      const session = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[3]);
      assert.equal(session, pid, "the daemon leads its own session");
    }
    const welcome = await hello(path.join(dir, "a.sock"));
    assert.deepEqual(welcome.result, { Ok: { data: { daemon_version: "stub-daemon", protocol_version: 26, pid } } });
    const commands = readFileSync(path.join(dir, "a.commands"), "utf8").trim().split("\n");
    assert.ok(commands.length >= 2 && commands.every((cmd) => cmd === "hello"), `Electron sent only hello: ${commands.join(", ")}`);
    assert.match(readFileSync(path.join(dir, "logs", "managed-start-aether.log"), "utf8"), /stub daemon \d+ listening/);
  } finally {
    if (pid !== undefined && Number.isSafeInteger(pid) && pid > 1) await stopStub(pid);
    rmSync(dir, { recursive: true, force: true });
  }
}

test("quitting Electron leaves a daemon it started running", { timeout: RUN_TIMEOUT_MS + 20_000 }, () => survives("quit"));

test("a crash of Electron leaves a daemon it started running", { timeout: RUN_TIMEOUT_MS + 20_000 }, () => survives("crash"));
