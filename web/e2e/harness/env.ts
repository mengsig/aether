// The hermetic environment both daemons get: every home, XDG, Aether and Archon directory inside
// one short temporary root, no inherited variable but the locale, and stubs ahead of real CLIs.

import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

/** Unix socket paths are limited to 108 bytes; the roots stay far below it. */
const SOCKET_PATH_MAX = 100;
/** The only variables taken from the caller: nothing that names a directory or a credential. */
const INHERITED = ["LANG", "LC_ALL", "TZ"] as const;
const SYSTEM_PATH = ["/usr/local/bin", "/usr/bin", "/bin"];
/**
 * CLIs a daemon could reach a real service or credential with. Each is shadowed by a stub that
 * fails; `tailscale` answers "nothing published", which lets Aether serve its mount.
 */
const REFUSED = ["claude", "codex", "gemini", "gh", "curl", "wget", "ssh"] as const;

export interface Layout {
  readonly root: string;
  readonly home: string;
  readonly bin: string;
  readonly aether: { readonly config: string; readonly data: string; readonly socket: string; readonly work: string; readonly log: string };
  readonly archon: { readonly config: string; readonly data: string; readonly socket: string; readonly log: string };
  readonly env: NodeJS.ProcessEnv;
}

/** The user's real home: scripts/e2e.sh gives the runner a private HOME and names the real one. */
export function userHome(): string {
  const home = process.env.E2E_USER_HOME ?? os.homedir();
  assert.ok(path.isAbsolute(home), `the user's home is absolute: ${home}`);
  return home;
}

/** Where stack roots are made: `E2E_TMP` from scripts/e2e.sh, else /tmp. Never under the user's home. */
export function tempBase(): string {
  const base = process.env.E2E_TMP ?? "/tmp";
  assert.ok(path.isAbsolute(base), `E2E_TMP is absolute: ${base}`);
  assert.ok(!`${base}/`.startsWith(`${userHome()}/`), `stack roots stay outside the user's home: ${base}`);
  return base;
}

function environment(layout: Omit<Layout, "env">): NodeJS.ProcessEnv {
  const xdg = path.join(layout.root, "xdg");
  const env: NodeJS.ProcessEnv = {
    PATH: [layout.bin, ...SYSTEM_PATH].join(":"),
    HOME: layout.home,
    TMPDIR: path.join(layout.root, "tmp"),
    XDG_CONFIG_HOME: path.join(xdg, "config"), XDG_DATA_HOME: path.join(xdg, "data"),
    XDG_CACHE_HOME: path.join(xdg, "cache"), XDG_STATE_HOME: path.join(xdg, "state"), XDG_RUNTIME_DIR: path.join(xdg, "run"),
    AETHER_CONFIG_DIR: layout.aether.config, AETHER_DATA_DIR: layout.aether.data, AETHER_SOCKET: layout.aether.socket,
    ARCHON_CONFIG_DIR: layout.archon.config, ARCHON_DATA_DIR: layout.archon.data, ARCHON_SOCKET: layout.archon.socket,
    ARCHON_NO_UPDATE_CHECK: "1", ARCHON_NO_WIZARD: "1", ARCHON_REMOTE: "0", NO_COLOR: "1",
    GIT_CONFIG_NOSYSTEM: "1", GIT_TERMINAL_PROMPT: "0",
  };
  for (const name of INHERITED) if (process.env[name] !== undefined) env[name] = process.env[name];
  return env;
}

async function writeStubs(bin: string): Promise<void> {
  await writeFile(path.join(bin, "tailscale"), "#!/bin/sh\n# e2e: nothing is published, so no Funnel.\nprintf '{}\\n'\n", { mode: 0o755 });
  for (const name of REFUSED) {
    await writeFile(path.join(bin, name), `#!/bin/sh\necho "${name} is not available in the e2e harness" >&2\nexit 127\n`, { mode: 0o755 });
  }
}

/** Makes a fresh root with every directory the daemons use; `HOME` is empty and read-only. */
export async function createLayout(): Promise<Layout> {
  const root = await mkdtemp(path.join(tempBase(), "ae-"));
  const layout: Omit<Layout, "env"> = {
    root,
    home: path.join(root, "home"),
    bin: path.join(root, "bin"),
    aether: { config: path.join(root, "a", "config"), data: path.join(root, "a", "data"), socket: path.join(root, "a.sock"), work: path.join(root, "work"), log: path.join(root, "aetherd.log") },
    archon: { config: path.join(root, "r", "config"), data: path.join(root, "r", "data"), socket: path.join(root, "r.sock"), log: path.join(root, "archond.log") },
  };
  for (const socket of [layout.aether.socket, layout.archon.socket]) {
    assert.ok(Buffer.byteLength(socket) < SOCKET_PATH_MAX, `socket path ${socket} is short enough for a Unix socket`);
  }
  const env = environment(layout);
  const dirs = [layout.home, layout.bin, layout.aether.config, layout.aether.data, layout.aether.work, layout.archon.config, layout.archon.data,
    String(env.TMPDIR), String(env.XDG_CONFIG_HOME), String(env.XDG_DATA_HOME), String(env.XDG_CACHE_HOME), String(env.XDG_STATE_HOME), String(env.XDG_RUNTIME_DIR)];
  for (const dir of dirs) await mkdir(dir, { recursive: true, mode: 0o700 });
  await writeStubs(layout.bin);
  await chmod(layout.home, 0o500);
  for (const [name, value] of Object.entries(env)) {
    if (/(^|_)(HOME|DIR|SOCKET)$/.test(name)) assert.ok(String(value).startsWith(`${root}/`), `${name} is inside the stack root: ${value}`);
  }
  return { ...layout, env };
}
