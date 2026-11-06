// Every Archon control the app shows reaches archond: the web packages' `archonAccess` and the
// bridge's pass list agree on every command, and each command the packages' code sends passes.

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { archonAccess, checkAllowed } from "../../../../packages/core/src/commands.ts";
import { WEB_ROOT } from "../../src/main/paths.ts";
import { ARCHON_APP_CLIENT, passThroughRefusal } from "../../src/security/classes.ts";
import { SCHEMAS } from "./support.ts";

const PACKAGES = path.join(WEB_ROOT, "packages");
const SKIPPED_DIRS = new Set(["fixture", "fixtures", "test", "node_modules"]);

/** The app's own form of a command with no arguments: a hello declares the web app. */
function bare(cmd: string): Record<string, unknown> {
  return cmd === "hello" ? { cmd, client: ARCHON_APP_CLIENT } : { cmd };
}

function uiSends(frame: Record<string, unknown>): boolean {
  return archonAccess(frame) !== "cli_only";
}

function bridgePasses(frame: Record<string, unknown>): boolean {
  return passThroughRefusal("archon", frame) === undefined;
}

/** The product sources of the web packages: no tests, fixtures or generated protocol. */
function productSources(dir: string, found: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const at = path.join(dir, entry.name);
    if (entry.isDirectory() && !SKIPPED_DIRS.has(entry.name)) productSources(at, found);
    else if (entry.isFile() && /\.tsx?$/.test(entry.name) && !entry.name.includes(".test.")) found.push(at);
  }
  return found;
}

/** The generated Archon builders, `issueItems` → `issue_items`. */
function archonBuilders(): Map<string, string> {
  const source = readFileSync(path.join(PACKAGES, "protocol/src/archon.ts"), "utf8");
  const builders = new Map<string, string>();
  for (const [, name, cmd] of source.matchAll(/export function (\w+)\([^)]*\): \w+ \{\n {2}return \{ cmd: "(\w+)"/g)) {
    builders.set(name ?? "", cmd ?? "");
  }
  assert.ok(builders.size > 50, `the parse found the builders (${builders.size})`);
  return builders;
}

/** Each command name the packages' code writes in a `cmd:` field or builds with `archon.<builder>(`. */
function sentCommands(): Map<string, string> {
  const builders = archonBuilders();
  const sent = new Map<string, string>();
  const protocol = path.join(PACKAGES, "protocol");
  for (const file of productSources(PACKAGES).filter((at) => !at.startsWith(protocol))) {
    const source = readFileSync(file, "utf8");
    const where = path.relative(WEB_ROOT, file);
    for (const [, value] of source.matchAll(/\bcmd:\s*([^,}\n]+)/g)) {
      for (const [, name] of (value ?? "").matchAll(/["'](\w+)["']/g)) sent.set(name ?? "", where);
    }
    for (const [, builder] of source.matchAll(/\barchon\.(\w+)\(/g)) {
      const cmd = builders.get(builder ?? "");
      if (cmd !== undefined) sent.set(cmd, where);
    }
  }
  return sent;
}

test("the web packages and the bridge agree on every Archon command", () => {
  const commands = SCHEMAS.frames.archon.commands();
  assert.ok(commands.length > 80, `the schema lists Archon's commands (${commands.length})`);
  for (const cmd of commands) {
    assert.equal(uiSends(bare(cmd)), bridgePasses(bare(cmd)), `${cmd}: the app's control and the bridge disagree`);
  }
});

test("the CLI's knobs are refused alike on both sides, and the soft forms pass", () => {
  const knobs = [
    { cmd: "pause", scope: "global", hard: true }, { cmd: "resume", scope: "global", hard: true },
    { cmd: "answer", question: "q_1", text: "yes", chat: true },
  ];
  for (const frame of knobs) {
    assert.ok(!uiSends(frame) && !bridgePasses(frame), JSON.stringify(frame));
    assert.throws(() => checkAllowed(archonAccess(frame), "local"), /archon CLI/);
  }
  for (const frame of [{ cmd: "pause", scope: "global", hard: false }, { cmd: "answer", question: "q_1", text: "yes" }]) {
    assert.ok(uiSends(frame) && bridgePasses(frame), JSON.stringify(frame));
  }
});

test("every Archon command the web packages send passes the bridge", () => {
  const archon = new Set(SCHEMAS.frames.archon.commands());
  // A name both daemons have is ambiguous here; the agreement test above covers its Archon side.
  const aether = new Set(SCHEMAS.frames.aether.commands());
  const sent = [...sentCommands()].filter(([cmd]) => archon.has(cmd) && !aether.has(cmd));
  const names = new Set(sent.map(([cmd]) => cmd));
  for (const wanted of ["say", "answer", "get_initiative", "enqueue", "issue_items"]) {
    assert.ok(names.has(wanted), `the scan finds ${wanted}`);
  }
  for (const [cmd, where] of sent) {
    assert.ok(bridgePasses(bare(cmd)), `${where} sends ${cmd}, which the desktop bridge refuses`);
  }
});
