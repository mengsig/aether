// Every Archon control the app shows reaches archond: the web packages' `archonAccess` and the
// bridge's pass list agree on every command, and each command the packages' code sends passes.

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { AETHER_HOST_READS as UI_HOST_READS, aetherAccess, archonAccess, checkAllowed } from "../../../../packages/core/src/commands.ts";
import { WEB_ROOT } from "../../src/main/paths.ts";
import { AETHER_HOST_READS, ARCHON_APP_CLIENT, passThroughRefusal } from "../../src/security/classes.ts";
import { SCHEMAS } from "./support.ts";

const PACKAGES = path.join(WEB_ROOT, "packages");
/** Everything of the app that can send a command: the packages and both app shells. */
const SOURCE_ROOTS = [PACKAGES, path.join(WEB_ROOT, "apps/web/src"), path.join(WEB_ROOT, "apps/desktop/src")];
const SKIPPED_DIRS = new Set(["fixture", "fixtures", "test", "node_modules"]);
/** Sources that talk only to archond: the Archon and GitHub packages, and every archon-* module. */
const ARCHON_OWNED = /^packages\/(archon|github)\/|\/archon-[\w-]+\.tsx?$/;

/** The app's own form of a command with no arguments: a hello declares the web app. */
function bare(cmd: string): Record<string, unknown> {
  return cmd === "hello" ? { cmd, client: ARCHON_APP_CLIENT } : { cmd };
}

function uiSends(frame: Record<string, unknown>): boolean {
  return archonAccess(frame) !== "cli_only";
}

function bridgePasses(frame: Record<string, unknown>): boolean {
  return passThroughRefusal("archon", frame, 26) === undefined;
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

/** Each command the packages' code writes in a `cmd:` field or builds with `archon.<builder>(`, with the files that do. */
function sentCommands(): Map<string, string[]> {
  const builders = archonBuilders();
  const sent = new Map<string, string[]>();
  const add = (cmd: string, where: string) => sent.set(cmd, [...(sent.get(cmd) ?? []), where]);
  const protocol = path.join(PACKAGES, "protocol");
  // privileged.ts builds main's own frames, sent only after the user confirms them; no renderer frame.
  const mainOwn = path.join(WEB_ROOT, "apps/desktop/src/bridge/privileged.ts");
  const files = SOURCE_ROOTS.flatMap((root) => productSources(root)).filter((at) => !at.startsWith(protocol) && at !== mainOwn);
  assert.ok(files.length > 100, `the scan reads the app's sources (${files.length})`);
  for (const file of files) {
    const source = readFileSync(file, "utf8");
    const where = path.relative(WEB_ROOT, file);
    for (const [, value] of source.matchAll(/\bcmd:\s*([^,}\n]+)/g)) {
      for (const [, name] of (value ?? "").matchAll(/["'](\w+)["']/g)) add(name ?? "", where);
    }
    for (const [, builder] of source.matchAll(/\barchon\.(\w+)\(/g)) {
      const cmd = builders.get(builder ?? "");
      if (cmd !== undefined) add(cmd, where);
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
  const aether = new Set(SCHEMAS.frames.aether.commands());
  // A name both daemons have counts where only archond hears it: an Archon-owned source.
  const sent = [...sentCommands()].filter(([cmd]) => archon.has(cmd)).flatMap(([cmd, files]) =>
    files.filter((where) => !aether.has(cmd) || ARCHON_OWNED.test(where)).map((where) => [cmd, where] as const));
  const names = new Set(sent.map(([cmd]) => cmd));
  for (const wanted of ["say", "answer", "get_initiative", "enqueue", "issue_items", "status", "pause", "read_artifact"]) {
    assert.ok(names.has(wanted), `the scan finds ${wanted}`);
  }
  for (const [cmd, where] of sent) {
    assert.ok(bridgePasses(bare(cmd)), `${where} sends ${cmd}, which the desktop bridge refuses`);
  }
});


test("the core host-read allowlist matches the bridge without widening the daemon class", () => {
  assert.deepEqual([...UI_HOST_READS], [...AETHER_HOST_READS]);
  assert.deepEqual([...AETHER_HOST_READS], ["get_remote"]);
  for (const cmd of UI_HOST_READS) {
    assert.equal(aetherAccess({ cmd }), "host_only");
    assert.equal(passThroughRefusal("aether", { cmd, req: 1 }), undefined);
    assert.notEqual(passThroughRefusal("aether", { cmd, req: 1 }, undefined, "subscribe"), undefined);
  }
});
