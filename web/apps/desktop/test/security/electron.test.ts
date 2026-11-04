// Runs the real main process headless (offscreen windows, the headless Ozone platform, no
// display) against the fixtures, and requires every named security check to pass.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const HARNESS = fileURLToPath(new URL("./electron/harness.ts", import.meta.url));
const RESULT_PREFIX = "DESKTOP-SECURITY-RESULTS ";
const RUN_TIMEOUT_MS = 120_000;

const EXPECTED = [
  "app window preferences", "window.desktop is the only bridge", "app:// serves web/dist byte for byte",
  "app:// refuses paths outside the bundle", "app window navigation is pinned", "app window opens no windows",
  "app CSP blocks the remote origin", "subframes get no bridge", "app window has no seen request at the remote origin",
  "bridge passes a status request", "bridge refuses shutdown, host-only and remote-admin commands",
  "privileged action sends the confirmed frame", "privileged action refused when cancelled",
  "openRemote refuses http, paths, userinfo and unlisted origins", "openRemote opens a listed origin",
  "remote window preferences", "remote window has its own partition",
  "remote window authenticates with Origin and a SameSite=Strict cookie", "remote window keeps its origin as its title",
  "app window has no request in the remote session", "remote navigation is pinned to its origin",
  "remote redirects are pinned to its origin", "remote window opens no windows", "remote window gets no permissions",
  "no IPC handler answers a remote window", "removing an origin clears its partition",
  "a remote window whose load fails is not kept",
];

interface CheckResult { name: string; ok: boolean; ms: number; error?: string }

function runHarness(dir: string): Promise<{ code: number | null; stdout: string; stderr: string }> {
  const electron = createRequire(import.meta.url)("electron") as unknown as string;
  assert.equal(typeof electron, "string", "the electron package names its binary");
  const env: NodeJS.ProcessEnv = { ...process.env, DESKTOP_SECURITY_DIR: dir };
  delete env.DISPLAY;
  delete env.WAYLAND_DISPLAY;
  delete env.ELECTRON_RUN_AS_NODE;
  const child = spawn(electron, ["--ozone-platform=headless", HARNESS], { env, stdio: ["ignore", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk: Buffer) => { stdout += chunk.toString("utf8"); });
  child.stderr.on("data", (chunk: Buffer) => { stderr += chunk.toString("utf8"); });
  const timer = setTimeout(() => child.kill("SIGKILL"), RUN_TIMEOUT_MS);
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code) => {
      clearTimeout(timer);
      resolve({ code, stdout, stderr });
    });
  });
}

test("the desktop app passes every headless security check", { timeout: RUN_TIMEOUT_MS + 10_000 }, async () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), "desktop-sec-"));
  try {
    const run = await runHarness(dir);
    const line = run.stdout.split("\n").find((text) => text.startsWith(RESULT_PREFIX));
    assert.ok(line !== undefined, `the harness reported no results (exit ${run.code}):\n${run.stderr.slice(-4000)}`);
    const results = JSON.parse(line.slice(RESULT_PREFIX.length)) as CheckResult[];
    const failed = results.filter((result) => !result.ok);
    assert.deepEqual(failed, [], failed.map((result) => `${result.name}: ${result.error}`).join("\n"));
    assert.deepEqual(results.map((result) => result.name), EXPECTED);
    assert.equal(run.code, 0);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
