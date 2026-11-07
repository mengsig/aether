// Electron never stops a daemon on its own: no source sends `shutdown`, kills or signals a process.
// Setup's `aether stop`, after the user confirms a restart, is the one stop command it runs.

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { LAUNCHERS } from "../../src/lifecycle/launch.ts";
import { AETHER_STOP_ARGS } from "../../src/lifecycle/setup.ts";
import { DESKTOP_ROOT } from "../../src/main/paths.ts";

const SOURCE = path.join(DESKTOP_ROOT, "src");
/**
 * The class allowlist lists daemon command names, `shutdown` among them only to refuse it, and
 * Aether's agent-level `restart`; test/security covers the refusal.
 */
const COMMAND_LIST = path.join(SOURCE, "security", "classes.ts");
/** Setup restarts Aether through `aether stop`, only once the native confirmation says yes. */
const SETUP = path.join(SOURCE, "lifecycle", "setup.ts");

// [what, pattern, whether the command list may hold it]
const STOPPING: ReadonlyArray<readonly [string, RegExp, boolean]> = [
  ["a kill call", /\.kill\s*\(|\bkillpg\b|\bpkill\b|\bkillall\b/, false],
  ["a signal name", /\bSIG(TERM|KILL|INT|HUP|QUIT|STOP|ABRT|USR1|USR2)\b/, false],
  ["a shutdown command", /["'`]shutdown["'`]|cmd:\s*["'`]shutdown/, true],
  ["a stop command", /["'`](stop|restart|daemon stop)["'`]/, true],
];

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const file = path.join(dir, entry.name);
    if (entry.isDirectory()) return sources(file);
    return /\.(ts|cjs|js|mjs)$/.test(entry.name) ? [file] : [];
  });
}

/** Code without comments, which may say what is never done. */
function code(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").split("\n").map((line) => line.replace(/(^|\s)\/\/.*$/, "")).join("\n");
}

test("no desktop source sends shutdown, kills or signals a process", () => {
  const files = sources(SOURCE);
  assert.ok(files.length > 20 && files.some((file) => file.includes(`${path.sep}lifecycle${path.sep}`)), "the sources were found");
  const found: string[] = [];
  for (const file of files) {
    const text = code(readFileSync(file, "utf8"));
    for (const [what, pattern, listed] of STOPPING) {
      if (listed && file === COMMAND_LIST) continue;
      if (what === "a stop command" && file === SETUP) continue;
      if (pattern.test(text)) found.push(`${path.relative(DESKTOP_ROOT, file)}: ${what}`);
    }
  }
  assert.deepEqual(found, []);
  assert.match(code(readFileSync(COMMAND_LIST, "utf8")), /if \(cmd === "shutdown"\) return "shutdown is never sent/);
});

test("setup's one stop command is aether stop, run only after a confirmed restart", () => {
  assert.deepEqual(AETHER_STOP_ARGS, ["stop"]);
  const setup = code(readFileSync(SETUP, "utf8"));
  assert.equal(setup.match(/["'`]stop["'`]/g)?.length, 1, "setup names the stop command once");
  assert.equal(setup.match(/AETHER_STOP_ARGS/g)?.length, 2, "declared once and run once");
  const confirmAt = setup.indexOf('confirmation(TEMPLATES["setup.restart"]');
  const stopAt = setup.indexOf('run("aether", AETHER_STOP_ARGS');
  assert.ok(confirmAt !== -1 && stopAt > confirmAt, "the stop runs after the restart is confirmed");
  assert.match(setup.slice(confirmAt, stopAt), /throw new BridgeRefusal\("Cancelled"\)/, "a cancel stops nothing");
});

test("managed start runs only each CLI's start command", () => {
  assert.deepEqual(LAUNCHERS, {
    aether: { command: "aether", args: ["daemon"] },
    archon: { command: "archon", args: ["daemon", "start"] },
  });
});
