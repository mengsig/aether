// aetherd and archond as a user runs them: foreground daemons configured by files, then the
// CLI to turn the listener on, mount archond at /archon on Aether's listener, and pair.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { promisify } from "node:util";
import type { Layout } from "./env.ts";
import { spawnLogged, stopGroup, tail, until, type Spawned } from "./process.ts";
import { socketRequest, type Command, type Reply } from "./socket.ts";

const run = promisify(execFile);
const START_MS = 30_000;
const CLI_MS = 30_000;
export const AETHER_PROTOCOL = 26;
export const FAUX_MODEL = "mock/mock-1";

export interface Ports { readonly aether: number; readonly archon: number }

/** Two distinct free loopback ports, held together so they differ; each is released before use. */
export async function freePorts(): Promise<Ports> {
  const servers = [net.createServer(), net.createServer()];
  try {
    for (const server of servers) await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    const [aether, archon] = servers.map((server) => (server.address() as net.AddressInfo).port);
    assert.ok(aether !== undefined && archon !== undefined && aether !== archon, "two distinct ports");
    return { aether, archon };
  } finally {
    for (const server of servers) await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

export async function cli(binary: string, args: readonly string[], layout: Layout): Promise<string> {
  try {
    const { stdout } = await run(binary, [...args], { env: layout.env, cwd: layout.aether.work, timeout: CLI_MS, maxBuffer: 4 * 1024 * 1024 });
    return stdout;
  } catch (error) {
    const failed = error as { stdout?: string; stderr?: string };
    throw new Error(`${path.basename(binary)} ${args.join(" ")} failed:\n${failed.stderr ?? ""}${failed.stdout ?? ""}`, { cause: error });
  }
}

async function logShows(log: string, text: string, from: number): Promise<boolean> {
  const content = await readFile(log, "utf8").catch(() => "");
  return content.slice(from).includes(text);
}

async function logSize(log: string): Promise<number> {
  return (await readFile(log, "utf8").catch(() => "")).length;
}

/** What differs between the two daemons: their name, files, hello and command line. */
interface DaemonSpec {
  readonly name: string;
  readonly log: string;
  readonly socket: string;
  readonly args: readonly string[];
  readonly hello: Command;
}

/** A daemon process with its socket protocol; `start` is repeatable, which is how a restart works. */
export class Daemon {
  readonly binary: string;
  readonly layout: Layout;
  readonly ports: Ports;
  readonly spec: DaemonSpec;
  #process: Spawned | undefined;

  constructor(binary: string, layout: Layout, ports: Ports, spec: DaemonSpec) {
    assert.ok(path.isAbsolute(binary), `the daemon binary path is absolute: ${binary}`);
    assert.ok(spec.log.startsWith(`${layout.root}/`) && spec.socket.startsWith(`${layout.root}/`), "a daemon's files are inside its root");
    this.binary = binary;
    this.layout = layout;
    this.ports = ports;
    this.spec = spec;
  }

  get pid(): number | undefined { return this.#process?.pid; }

  async start(): Promise<void> {
    assert.equal(this.#process, undefined, `${this.spec.name} is not already running`);
    const { log, name } = this.spec;
    const from = await logSize(log);
    const started = spawnLogged(this.binary, this.spec.args, { env: this.layout.env, cwd: this.layout.aether.work, log });
    this.#process = started;
    try {
      await until(async () => started.child.exitCode === null && await logShows(log, "listening", from), Boolean, `${name} to listen`, START_MS);
      await until(() => this.request({ cmd: "status" }).then(() => true, () => false), Boolean, `${name} to answer on its socket`, START_MS);
    } catch (error) {
      await this.stop();
      throw new Error(`${name} did not start. Its log:\n${await tail(log)}`, { cause: error });
    }
  }

  async stop(): Promise<void> {
    const running = this.#process;
    if (running === undefined) return;
    this.#process = undefined;
    await stopGroup(running);
  }

  async request(command: Command): Promise<Reply> {
    return (await socketRequest(this.spec.socket, this.spec.hello, command)).reply;
  }

  async welcome(): Promise<Reply> {
    return (await socketRequest(this.spec.socket, this.spec.hello, { cmd: "status" })).welcome;
  }
}

export class Aetherd extends Daemon {
  constructor(binary: string, layout: Layout, ports: Ports) {
    super(binary, layout, ports, {
      name: "aetherd", log: layout.aether.log, socket: layout.aether.socket, args: ["--local", "daemon", "--foreground"],
      hello: { cmd: "hello", client_name: "aether-web-e2e", client_version: "0.0.0", protocol_version: AETHER_PROTOCOL, features: [] },
    });
  }

  /** The listener's loopback origin, which the browser loads the app from. */
  get origin(): string { return `http://127.0.0.1:${this.ports.aether}`; }

  async configure(providers: string): Promise<void> {
    await writeFile(path.join(this.layout.aether.config, "providers.toml"), providers);
    await writeFile(path.join(this.layout.aether.config, "config.toml"),
      `[model]\ndefault = "${FAUX_MODEL}"\n\n[remote]\naddress = "127.0.0.1:${this.ports.aether}"\n`);
  }

  /** Once, after the first start: the listener on, and archond mounted at /archon. Both persist. */
  async wire(): Promise<void> {
    await cli(this.binary, ["--local", "remote", "enable"], this.layout);
    await cli(this.binary, ["--local", "remote", "mount", "add", "/archon", "--target", `http://127.0.0.1:${this.ports.archon}`, "--kind", "archon"], this.layout);
  }

  async pairing(role: "operator" | "observer"): Promise<{ id: string; code: string; url: string }> {
    const { reply, data } = await this.request({ cmd: "start_pairing", role });
    assert.equal(reply, "pairing");
    assert.equal(data.state, "open");
    return { id: String(data.id), code: String(data.code), url: String(data.url) };
  }
}

export interface ArchonSchema { readonly protocol_version: number; readonly features: readonly string[] }

/** archond takes at most 3 pairing claims per 10 s, daemon-wide (archon-remote's GuessLimit). */
const CLAIMS_PER_WINDOW = 3;
const CLAIM_WINDOW_MS = 10_500;

export class Archond extends Daemon {
  readonly schema: ArchonSchema;
  #claims: number[] = [];

  constructor(binary: string, layout: Layout, ports: Ports, schema: ArchonSchema) {
    assert.ok(Number.isInteger(schema.protocol_version) && schema.features.length > 0, "the vendored schema names a version and features");
    super(binary, layout, ports, {
      name: "archond", log: layout.archon.log, socket: layout.archon.socket, args: ["daemon", "run"],
      hello: { cmd: "hello", protocol_version: schema.protocol_version, client: "archon-cli", client_version: "e2e" },
    });
    this.schema = schema;
  }

  /** Shared mode on the harness's aetherd, the faux model only, served behind Aether's /archon mount. */
  async configure(aetherOrigin: string, aetherSocket: string): Promise<void> {
    assert.match(aetherOrigin, /^http:\/\/127\.0\.0\.1:\d+$/, "archond trusts only a loopback Aether origin");
    await writeFile(path.join(this.layout.archon.config, "config.toml"), [
      "[aether]", 'mode = "shared"', `socket = "${aetherSocket}"`, "",
      "[orchestrator]", `model = "${FAUX_MODEL}"`, "",
      "[models]", `allow = ["${FAUX_MODEL}"]`, "",
      "[remote]", `address = "127.0.0.1:${this.ports.archon}"`, 'mounted_at = "/archon"', `trusted_origins = ["${aetherOrigin}"]`, "",
    ].join("\n"));
  }

  /** Once, after the first start: the listener on; remote.json keeps it on across restarts. */
  async wire(): Promise<void> {
    await cli(this.binary, ["remote", "enable"], this.layout);
  }

  /** Fails unless archond speaks exactly the vendored schema's protocol and features. */
  async checkProtocol(): Promise<void> {
    const welcome = await this.welcome();
    assert.equal(welcome.reply, "welcome", `archond greets with welcome: ${JSON.stringify(welcome)}`);
    const spoken = welcome.data.protocol_version;
    const features = [...(welcome.data.features as string[] | undefined ?? [])].sort();
    const expected = [...this.schema.features].sort();
    if (spoken !== this.schema.protocol_version || JSON.stringify(features) !== JSON.stringify(expected)) {
      throw new Error(`ARCHOND_BIN speaks protocol ${String(spoken)} with features [${features.join(", ")}], but web/schemas/archon.protocol.schema.json `
        + `is protocol ${this.schema.protocol_version} with [${expected.join(", ")}]. Build archond from the commit the schema was vendored from.`);
    }
  }

  /** Waits until one more claim fits archond's guess window, and counts it; a restart clears the window. */
  async claimSlot(): Promise<void> {
    const now = Date.now();
    this.#claims = this.#claims.filter((at) => now - at < CLAIM_WINDOW_MS);
    if (this.#claims.length >= CLAIMS_PER_WINDOW) {
      const wait = (this.#claims[0] as number) + CLAIM_WINDOW_MS - now;
      await new Promise((resolve) => setTimeout(resolve, wait));
    }
    this.#claims.push(Date.now());
    assert.ok(this.#claims.length <= CLAIMS_PER_WINDOW + 1, "the claim window never overfills");
  }

  override async start(): Promise<void> {
    this.#claims = [];
    await super.start();
  }

  async pairing(): Promise<{ code: string }> {
    const { reply, data } = await this.request({ cmd: "remote_pair" });
    assert.equal(reply, "remote_pairing", `remote_pair answers remote_pairing: ${reply}`);
    assert.match(String(data.code), /^[0-9A-Z]{4}-[0-9A-Z]{4}$/);
    return { code: String(data.code) };
  }
}
