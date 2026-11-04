// Discovery finds each socket by its CLI's rules (aether-protocol and archon-core paths.rs over
// the `dirs` crate), and health comes from one hello: the table pins every rule and override.

import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { helloFrame } from "../../src/bridge/bridge.ts";
import { findDaemons } from "../../src/discovery/daemons.ts";
import { HELLO_REQ, probe } from "../../src/discovery/health.ts";
import { machineOf, socketPath, type Env, type Platform } from "../../src/discovery/socket-paths.ts";

const PW = "/home/pw";

// [rule, platform, environment, aether socket, archon socket]
const RULES: ReadonlyArray<readonly [string, Platform, Env, string, string]> = [
  ["runtime dir", "linux", { HOME: "/h", XDG_RUNTIME_DIR: "/run/user/1000" },
    "/run/user/1000/aether/aetherd.sock", "/run/user/1000/archon/archond.sock"],
  ["socket variable beats the runtime dir", "linux", { XDG_RUNTIME_DIR: "/run/u", AETHER_SOCKET: "/s/a.sock", ARCHON_SOCKET: "/s/r.sock" },
    "/s/a.sock", "/s/r.sock"],
  ["each socket variable moves only its daemon", "linux", { XDG_RUNTIME_DIR: "/run/u", ARCHON_SOCKET: "/s/r.sock" },
    "/run/u/aether/aetherd.sock", "/s/r.sock"],
  ["an empty socket variable is still set", "linux", { XDG_RUNTIME_DIR: "/run/u", AETHER_SOCKET: "" },
    "", "/run/u/archon/archond.sock"],
  ["a relative socket variable is used as written", "linux", { AETHER_SOCKET: "rel/a.sock" },
    "rel/a.sock", `${PW}/.local/share/archon/archond.sock`],
  ["the runtime dir beats the data dir variable", "linux", { XDG_RUNTIME_DIR: "/run/u", AETHER_DATA_DIR: "/d", ARCHON_DATA_DIR: "/e" },
    "/run/u/aether/aetherd.sock", "/run/u/archon/archond.sock"],
  ["a trailing slash joins once", "linux", { XDG_RUNTIME_DIR: "/run/u/" },
    "/run/u/aether/aetherd.sock", "/run/u/archon/archond.sock"],
  ["a relative runtime dir is ignored", "linux", { HOME: "/h", XDG_RUNTIME_DIR: "run/u" },
    "/h/.local/share/aether/aetherd.sock", "/h/.local/share/archon/archond.sock"],
  ["an empty runtime dir is ignored", "linux", { HOME: "/h", XDG_RUNTIME_DIR: "" },
    "/h/.local/share/aether/aetherd.sock", "/h/.local/share/archon/archond.sock"],
  ["the data dir variable, with no runtime dir", "linux", { HOME: "/h", AETHER_DATA_DIR: "/d", ARCHON_DATA_DIR: "/e" },
    "/d/aetherd.sock", "/e/archond.sock"],
  ["an empty data dir variable is still set", "linux", { HOME: "/h", AETHER_DATA_DIR: "" },
    "aetherd.sock", "/h/.local/share/archon/archond.sock"],
  ["paths are joined, never normalized", "linux", { AETHER_DATA_DIR: "/d/../x", HOME: "/h//" },
    "/d/../x/aetherd.sock", "/h//.local/share/archon/archond.sock"],
  ["XDG_DATA_HOME", "linux", { HOME: "/h", XDG_DATA_HOME: "/xdg" },
    "/xdg/aether/aetherd.sock", "/xdg/archon/archond.sock"],
  ["a relative XDG_DATA_HOME is ignored", "linux", { HOME: "/h", XDG_DATA_HOME: "xdg" },
    "/h/.local/share/aether/aetherd.sock", "/h/.local/share/archon/archond.sock"],
  ["no HOME: the password database", "linux", {},
    `${PW}/.local/share/aether/aetherd.sock`, `${PW}/.local/share/archon/archond.sock`],
  ["an empty HOME: the password database", "linux", { HOME: "" },
    `${PW}/.local/share/aether/aetherd.sock`, `${PW}/.local/share/archon/archond.sock`],
  ["macOS has no runtime dir", "darwin", { HOME: "/Users/u", XDG_RUNTIME_DIR: "/run/u" },
    "/Users/u/Library/Application Support/aether/aetherd.sock", "/Users/u/Library/Application Support/archon/archond.sock"],
  ["macOS ignores XDG_DATA_HOME", "darwin", { HOME: "/Users/u", XDG_DATA_HOME: "/xdg" },
    "/Users/u/Library/Application Support/aether/aetherd.sock", "/Users/u/Library/Application Support/archon/archond.sock"],
  ["macOS socket and data dir variables", "darwin", { HOME: "/Users/u", AETHER_SOCKET: "/s/a.sock", ARCHON_DATA_DIR: "/e" },
    "/s/a.sock", "/e/archond.sock"],
];

test("socket discovery follows every CLI rule", () => {
  assert.equal(new Set(RULES.map(([rule]) => rule)).size, RULES.length, "each rule is named once");
  for (const [rule, platform, env, aether, archon] of RULES) {
    const machine = machineOf(env, platform, PW);
    assert.equal(socketPath("aether", machine), aether, `aether: ${rule}`);
    assert.equal(socketPath("archon", machine), archon, `archon: ${rule}`);
  }
});

test("discovery runs only where the CLIs say where sockets are", () => {
  assert.throws(() => machineOf({}, "win32", PW), /does not find daemons on win32/);
  assert.throws(() => machineOf({}, "linux", "relative/home"), /absolute/);
});

interface Stub {
  readonly socket: string;
  readonly seen: string[];
  close(): Promise<void>;
}

/** A Unix socket server that answers each line with `answer`, or stays silent when it gives undefined. */
function stubServer(dir: string, name: string, answer: (frame: { cmd: string; req: number }) => object | undefined): Promise<Stub> {
  const socket = path.join(dir, name);
  const seen: string[] = [];
  const open = new Set<net.Socket>();
  const server = net.createServer((connection) => {
    open.add(connection);
    connection.on("close", () => open.delete(connection));
    let buffered = "";
    connection.on("data", (chunk) => {
      buffered += chunk.toString("utf8");
      for (let newline = buffered.indexOf("\n"); newline !== -1; newline = buffered.indexOf("\n")) {
        const frame = JSON.parse(buffered.slice(0, newline)) as { cmd: string; req: number };
        buffered = buffered.slice(newline + 1);
        seen.push(frame.cmd);
        const result = answer(frame);
        if (result !== undefined) connection.write(`${JSON.stringify({ frame: "reply", id: frame.req, result })}\n`);
      }
    });
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socket, () => resolve({
      socket, seen, close: () => new Promise((done) => {
        for (const connection of open) connection.destroy();
        server.close(() => done());
      }),
    }));
  });
}

const HELLO = helloFrame("aether", 26, "0.0.0");

test("health is one hello: version from the welcome, an error reply still reachable, silence not", async () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), "e2-probe-"));
  const welcome = await stubServer(dir, "ok.sock", () => ({ Ok: { data: { daemon_version: "1.2.3", pid: 7 } } }));
  const refusing = await stubServer(dir, "err.sock", () => ({ Err: { code: "protocol_mismatch", message: "too old" } }));
  const silent = await stubServer(dir, "mute.sock", () => undefined);
  const hostile = await stubServer(dir, "odd.sock", () => ({ Ok: { data: { daemon_version: "1.0\n/run/x.sock" } } }));
  try {
    assert.equal(HELLO.req, HELLO_REQ);
    assert.deepEqual(await probe(welcome.socket, { hello: HELLO, timeoutMs: 2_000 }), { reachable: true, version: "1.2.3" });
    assert.deepEqual(await probe(refusing.socket, { hello: HELLO, timeoutMs: 2_000 }), { reachable: true });
    assert.deepEqual(await probe(silent.socket, { hello: HELLO, timeoutMs: 200 }), { reachable: false });
    assert.deepEqual(await probe(hostile.socket, { hello: HELLO, timeoutMs: 2_000 }), { reachable: true }, "an odd version is not shown");
    assert.deepEqual(await probe(path.join(dir, "absent.sock"), { hello: HELLO, timeoutMs: 2_000 }), { reachable: false });
    assert.deepEqual(await probe(path.join(dir, "absent", "x.sock"), { hello: HELLO, timeoutMs: 2_000 }), { reachable: false });
    assert.deepEqual(await probe("", { hello: HELLO, timeoutMs: 2_000 }), { reachable: false });
    for (const stub of [welcome, refusing, silent, hostile]) assert.deepEqual(stub.seen, ["hello"], "a probe sends hello and nothing else");
    await assert.rejects(probe(welcome.socket, { hello: { cmd: "shutdown", req: HELLO_REQ }, timeoutMs: 2_000 }), /only hello/);
    assert.deepEqual(welcome.seen, ["hello"]);
  } finally {
    await Promise.all([welcome, refusing, silent, hostile].map((stub) => stub.close()));
    rmSync(dir, { recursive: true, force: true });
  }
});

test("findDaemons lists both kinds with health, from the CLI paths", async () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), "e2-find-"));
  const aether = await stubServer(dir, "a.sock", () => ({ Ok: { data: { daemon_version: "9.9.9" } } }));
  try {
    const machine = machineOf({ AETHER_SOCKET: aether.socket, ARCHON_SOCKET: path.join(dir, "r.sock") }, "linux", PW);
    const found = await findDaemons(machine, (kind) => helloFrame(kind, kind === "aether" ? 26 : 24, "0.0.0"));
    assert.deepEqual(found, [
      { id: "aether:local", kind: "aether", label: "aetherd", socket: aether.socket, reachable: true, version: "9.9.9" },
      { id: "archon:local", kind: "archon", label: "archond", socket: path.join(dir, "r.sock"), reachable: false },
    ]);
  } finally {
    await aether.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
