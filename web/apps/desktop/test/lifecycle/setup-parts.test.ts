// The pieces of setting up this computer: version parsing, Archon's config file, the answer of
// `archon setup --json`, the install directories on PATH, the command runner and the validator.
// Nothing here runs the real CLIs or touches the real home: stubs live in temporary directories.

import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { machineOf } from "../../src/discovery/socket-paths.ts";
import { ARCHON_CONFIG_TEXT, archonConfigFile, ensureArchonConfig, parseArchonSetup } from "../../src/lifecycle/archon-connect.ts";
import { commandRunner, firstLine, type CommandResult, type CommandRunner } from "../../src/lifecycle/commands.ts";
import { installDirs, withInstallDirs } from "../../src/lifecycle/install-path.ts";
import { toolContext } from "../../src/lifecycle/managed-start.ts";
import { findTools, parseVersion } from "../../src/lifecycle/tools.ts";
import { BridgeRefusal } from "../../src/security/validate.ts";
import { APP_SENDER, CONNECTIONS, VALIDATOR } from "../security/support.ts";

function tempDir(): { dir: string; cleanup(): void } {
  const dir = mkdtempSync(path.join(os.tmpdir(), "e2-setup-"));
  return { dir, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

const ran = (stdout: string, code = 0): CommandResult => ({ found: true, code, stdout, stderr: "" });

function fakeRunner(answers: Readonly<Record<string, CommandResult>>): CommandRunner {
  return async (tool, args) => {
    const answer = answers[`${tool} ${args.join(" ")}`];
    if (answer === undefined) throw new Error(`unexpected command: ${tool} ${args.join(" ")}`);
    return answer;
  };
}

test("each CLI's version line gives its version, and anything else gives none", () => {
  assert.equal(parseVersion("aether", "aether 1.3.0 (protocol 24)\n"), "1.3.0");
  assert.equal(parseVersion("aether", "aether 1.3.0-rc.1\n"), "1.3.0-rc.1");
  assert.equal(parseVersion("archon", "archon 1.4.12\n"), "1.4.12");
  for (const [tool, text] of [["aether", "archon 1.4.12"], ["archon", "archon"], ["archon", "archon 1.4 extra"],
    ["aether", "aether 1.3.0 (protocol x)"], ["aether", ""], ["archon", "archon <script>"]] as const) {
    assert.equal(parseVersion(tool, text), undefined, `${tool}: ${text}`);
  }
});

test("the tools step finds both CLIs, a missing one as not found, a broken one without a version", async () => {
  const both = await findTools(fakeRunner({ "aether version": ran("aether 1.3.0 (protocol 24)\n"), "archon --version": ran("archon 1.4.12\n") }));
  assert.deepEqual(both, [{ name: "aether", found: true, version: "1.3.0" }, { name: "archon", found: true, version: "1.4.12" }]);
  const partial = await findTools(fakeRunner({ "aether version": ran("", 2), "archon --version": { found: false } }));
  assert.deepEqual(partial, [{ name: "aether", found: true }, { name: "archon", found: false }]);
});

test("Archon's config file is where archon-core reads it", () => {
  const linux = (env: Record<string, string>) => archonConfigFile(machineOf(env, "linux", "/home/pw"));
  assert.equal(linux({ HOME: "/home/u" }), "/home/u/.config/archon/config.toml");
  assert.equal(linux({ HOME: "/home/u", XDG_CONFIG_HOME: "/xdg" }), "/xdg/archon/config.toml");
  assert.equal(linux({ HOME: "/home/u", XDG_CONFIG_HOME: "rel" }), "/home/u/.config/archon/config.toml", "a relative XDG var is ignored");
  assert.equal(linux({ HOME: "" }), "/home/pw/.config/archon/config.toml", "an empty HOME means the password database's");
  assert.equal(linux({ HOME: "/home/u", ARCHON_CONFIG_DIR: "/cfg" }), "/cfg/config.toml");
  assert.equal(archonConfigFile(machineOf({ HOME: "/Users/u", XDG_CONFIG_HOME: "/xdg" }, "darwin", "/Users/pw")),
    "/Users/u/Library/Application Support/archon/config.toml");
  assert.throws(() => linux({ ARCHON_CONFIG_DIR: "rel" }), /absolute in the launch environment/);
  const context = toolContext(machineOf({ HOME: "/home/u", ARCHON_CONFIG_DIR: "rel" }, "linux", "/home/pw"), "/main", () => false);
  assert.equal(archonConfigFile(machineOf(context.env, "linux", "/home/pw")), "/main/rel/config.toml", "relative to main, as launched");
});

test("Archon's config is written once, exclusively and privately, and never overwritten", () => {
  const { dir, cleanup } = tempDir();
  try {
    const file = path.join(dir, "a", "archon", "config.toml");
    assert.equal(ensureArchonConfig(file), "written");
    assert.equal(readFileSync(file, "utf8"), ARCHON_CONFIG_TEXT);
    assert.match(ARCHON_CONFIG_TEXT, /^# .*desktop app.*\n\[aether\]\nmode = "shared"\n$/);
    assert.equal(statSync(file).mode & 0o777, 0o600);
    assert.equal(statSync(path.dirname(file)).mode & 0o777, 0o700);
    writeFileSync(file, "# mine\n");
    assert.equal(ensureArchonConfig(file), "kept");
    assert.equal(readFileSync(file, "utf8"), "# mine\n", "an existing config is left as it is");
    const linked = path.join(dir, "b", "config.toml");
    mkdirSync(path.dirname(linked));
    symlinkSync(path.join(dir, "elsewhere"), linked);
    assert.equal(ensureArchonConfig(linked), "kept", "a dangling symlink is not followed");
    writeFileSync(path.join(dir, "file"), "");
    assert.throws(() => ensureArchonConfig(path.join(dir, "file", "config.toml")), BridgeRefusal);
  } finally {
    cleanup();
  }
});

const health = (level: string) => ({ check: "mcp health", level, detail: "aetherd has not loaded the archon server" });

test("archon setup's JSON is accepted in both modes and refused when malformed", () => {
  const shared = { mode: "shared", config: "/c/config.toml", registration: "added", command: ["archon", "mcp"], mcp_health: health("fail") };
  assert.deepEqual(parseArchonSetup(JSON.stringify(shared)), { mode: "shared", registration: "added", health: "fail" });
  assert.deepEqual(parseArchonSetup(JSON.stringify({ ...shared, registration: "already_present", mcp_health: health("ok") })),
    { mode: "shared", registration: "already_present", health: "ok" });
  const dedicated = { mode: "dedicated", config: "/c/config.toml", mcp_health: health("warn") };
  assert.deepEqual(parseArchonSetup(JSON.stringify(dedicated)), { mode: "dedicated", registration: "dedicated", health: "warn" });
  for (const bad of ["", "not json", "[]", "null", JSON.stringify({ ...shared, mode: "other" }),
    JSON.stringify({ ...shared, registration: undefined }), JSON.stringify({ ...shared, registration: "dedicated" }),
    JSON.stringify({ ...shared, config: 3 }), JSON.stringify({ ...shared, mcp_health: undefined }),
    JSON.stringify({ ...shared, mcp_health: { ...health("ok"), level: "fine" } }),
    JSON.stringify({ ...shared, mcp_health: { ...health("ok"), check: "other" } }),
    JSON.stringify({ ...dedicated, mcp_health: { level: "ok" } })]) {
    assert.throws(() => parseArchonSetup(bad), (error: unknown) =>
      error instanceof BridgeRefusal && /could not connect Archon's tools/.test(error.message), bad);
  }
});

test("install directories are appended once each, only when they exist and are not on PATH", () => {
  const exists = (dir: string) => dir !== "/home/u/.cargo/bin";
  assert.deepEqual(installDirs("darwin", "/Users/u").at(-1), "/opt/homebrew/bin");
  assert.ok(!installDirs("linux", "/home/u").includes("/opt/homebrew/bin"));
  assert.deepEqual(withInstallDirs({ PATH: "/usr/local/bin:/usr/bin", HOME: "/home/u" }, "linux", "/home/u", exists),
    { PATH: "/usr/local/bin:/usr/bin:/home/u/.local/bin", HOME: "/home/u" }, "appended, never ahead, never twice");
  const full = { PATH: "/home/u/.local/bin:/usr/local/bin" };
  assert.equal(withInstallDirs(full, "linux", "/home/u", exists), full, "nothing to add leaves env as it is");
  assert.deepEqual(withInstallDirs({}, "darwin", "/Users/u", () => true),
    { PATH: "/usr/bin:/bin:/Users/u/.local/bin:/Users/u/.cargo/bin:/usr/local/bin:/opt/homebrew/bin" }, "an unset PATH keeps the default");
  assert.equal(withInstallDirs({ PATH: "/bin" }, "linux", "/home/u", () => false).PATH, "/bin");
  assert.throws(() => installDirs("linux", "home"), /absolute home/);
  const context = toolContext(machineOf({ PATH: "/bin", HOME: "u" }, "linux", "/home/pw"), "/main", (dir) => dir === "/main/u/.local/bin");
  assert.deepEqual(context, { env: { PATH: "/bin:/main/u/.local/bin", HOME: "/main/u" }, cwd: "/main/u" }, "launchers get the same PATH");
});

test("the runner runs a CLI without a shell, in home with the given env, and bounds its time", async () => {
  const { dir, cleanup } = tempDir();
  try {
    const bin = path.join(dir, "bin");
    mkdirSync(bin);
    const script = path.join(bin, "aether");
    writeFileSync(script, `#!/bin/sh\nif [ "$1" = slow ]; then exec /bin/sleep 5; fi\necho "$PWD $MARK $*"; echo oops >&2; exit 3\n`);
    chmodSync(script, 0o755);
    const run = commandRunner({ env: { PATH: bin, MARK: "m" }, cwd: dir });
    assert.deepEqual(await run("aether", ["a b", "$HOME"], 5_000), { found: true, code: 3, stdout: `${dir} m a b $HOME\n`, stderr: "oops\n" });
    assert.deepEqual(await run("archon", ["--version"], 5_000), { found: false }, "a missing CLI is not found");
    const started = Date.now();
    await assert.rejects(run("aether", ["slow"], 200), (error: unknown) => error instanceof BridgeRefusal && /did not answer/.test(error.message));
    assert.ok(Date.now() - started < 3_000, "the deadline ends the command");
    assert.equal(firstLine("\n  first  \nsecond"), "first");
  } finally {
    cleanup();
  }
});

test("the validator passes the three setup steps and refuses anything else", () => {
  for (const step of ["tools", "connect", "start"]) {
    assert.deepEqual(VALIDATOR.check(APP_SENDER, "setup", [step], CONNECTIONS), { method: "setup", step });
  }
  for (const args of [[], ["stop"], ["Tools"], [1], ["tools", "extra"], [{ step: "tools" }]]) {
    assert.throws(() => VALIDATOR.check(APP_SENDER, "setup", args, CONNECTIONS), BridgeRefusal, JSON.stringify(args));
  }
  assert.throws(() => VALIDATOR.check({ ...APP_SENDER, isAppWindow: false }, "setup", ["tools"], CONNECTIONS), BridgeRefusal);
});
