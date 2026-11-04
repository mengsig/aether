// Smoke-tests a packaged app (docs/desktop.md):
//   node build/smoke.ts --exec <app binary> --resources <its resources dir> --signing unsigned|signed
// The package must carry web/dist byte for byte; the app must start, load app://, show the
// shared frontend with window.desktop, report its signing, and exit 0. Run it under a display
// (xvfb-run in CI). It points discovery at absent sockets, so it never reaches a real daemon.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { parseArgs } from "node:util";
import { AETHER_SCHEMA, ARCHON_SCHEMA, WEB_DIST } from "../src/main/paths.ts";
import { SMOKE_ENV, SMOKE_PREFIX } from "../src/main/smoke.ts";
import { APP_ENTRY } from "../src/security/csp.ts";
import { bundleFiles } from "../test/security/bundle-files.ts";

const TIMEOUT_MS = 90_000;

/** The packaged bundle and schemas are the committed ones, file for file and byte for byte. */
export function checkSameBundle(resources: string): number {
  const packaged = path.join(resources, "web", "dist");
  const files = bundleFiles(WEB_DIST);
  assert.ok(files.includes("index.html"), "web/dist holds the app");
  assert.deepEqual(bundleFiles(packaged).sort(), [...files].sort(), "the package holds exactly web/dist's files");
  for (const file of files) {
    assert.ok(readFileSync(path.join(packaged, file)).equals(readFileSync(path.join(WEB_DIST, file))), `${file} differs`);
  }
  for (const [name, source] of [["protocol.schema.json", AETHER_SCHEMA], ["archon.protocol.schema.json", ARCHON_SCHEMA]] as const) {
    assert.ok(readFileSync(path.join(resources, "schemas", name)).equals(readFileSync(source)), `${name} differs`);
  }
  return files.length;
}

interface Run { readonly code: number | null; readonly signal: NodeJS.Signals | null; readonly stdout: string; readonly stderr: string }

/** Starts the app in a scratch home and waits for it to exit, killing its group after the timeout. */
function launch(exec: string, scratch: string): Promise<Run> {
  const env = {
    ...process.env,
    [SMOKE_ENV]: "1",
    HOME: path.join(scratch, "home"),
    XDG_CONFIG_HOME: path.join(scratch, "config"),
    XDG_RUNTIME_DIR: path.join(scratch, "run"),
    AETHER_SOCKET: path.join(scratch, "absent-aetherd.sock"),
    ARCHON_SOCKET: path.join(scratch, "absent-archond.sock"),
  };
  return new Promise((resolve, reject) => {
    const child = spawn(exec, [], { env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk: Buffer) => { stdout += chunk.toString(); });
    child.stderr.on("data", (chunk: Buffer) => { stderr += chunk.toString(); });
    const timer = setTimeout(() => {
      if (child.pid !== undefined) process.kill(-child.pid, "SIGKILL");
      reject(new Error(`The app did not exit within ${TIMEOUT_MS} ms\n${stdout}\n${stderr}`));
    }, TIMEOUT_MS);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(new Error(`Could not start ${exec}`, { cause: error }));
    });
    child.once("close", (code, signal) => {
      clearTimeout(timer);
      resolve({ code, signal, stdout, stderr });
    });
  });
}

function checkReport(run: Run, signing: string): unknown {
  const line = run.stdout.split("\n").find((text) => text.startsWith(SMOKE_PREFIX));
  assert.ok(line !== undefined, `the app printed no smoke report\n${run.stdout}\n${run.stderr}`);
  assert.equal(run.signal, null, "the app exits by itself");
  assert.equal(run.code, 0, `the app exits cleanly\n${run.stderr}`);
  const report = JSON.parse(line.slice(SMOKE_PREFIX.length)) as {
    url: string; page: { title: string; root: boolean; bridge: unknown }; packaged: boolean; build: { label: string; signed: boolean };
  };
  assert.equal(report.url, APP_ENTRY, "the app window loaded app://");
  // The shared frontend titles the page "<screen> · Aether" once it has rendered.
  assert.match(report.page.title, /^(.+ · )?Aether$/, "the shared frontend rendered");
  assert.deepEqual({ root: report.page.root, bridge: report.page.bridge }, { root: true, bridge: 1 }, "window.desktop is there");
  assert.equal(report.packaged, true, "a packaged app ran");
  assert.equal(report.build.label, `${signing} build`, "the build reports its signing");
  assert.equal(report.build.signed, signing === "signed");
  return report;
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { exec: { type: "string" }, resources: { type: "string" }, signing: { type: "string" } } });
  const { exec, resources, signing } = values;
  if (exec === undefined || resources === undefined) throw new Error("usage: smoke.ts --exec <binary> --resources <dir> --signing <s>");
  if (signing !== "unsigned" && signing !== "signed") throw new Error("--signing is unsigned or signed");
  console.log(`same bundle: ${checkSameBundle(resources)} web/dist files and both schemas match byte for byte`);
  const scratch = mkdtempSync(path.join(os.tmpdir(), "aether-smoke-"));
  try {
    const report = checkReport(await launch(exec, scratch), signing);
    console.log(`launch: ${JSON.stringify(report)}`);
    console.log("smoke: ok");
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

if (path.resolve(process.argv[1] ?? "") === import.meta.filename) {
  main().catch((error: unknown) => {
    console.error("Smoke test failed:", error);
    process.exit(1);
  });
}
