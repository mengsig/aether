// The whole system under test: a faux engine, aetherd and archond in one fresh root, wired as
// a user wires them. start, restart one daemon, and stop; stop leaves nothing behind.

import assert from "node:assert/strict";
import { access, constants, readFile, realpath, rm } from "node:fs/promises";
import path from "node:path";
import { Aetherd, Archond, cli, freePorts, type ArchonSchema } from "./daemons.ts";
import { providersToml, startEngine, type FauxEngine } from "./engine.ts";
import { createLayout, type Layout } from "./env.ts";
import { record } from "./process.ts";

export const ARCHON_SCHEMA = path.resolve(import.meta.dirname, "..", "..", "schemas", "archon.protocol.schema.json");

export interface Binaries { readonly aether: string; readonly archon: string }

export interface Stack {
  readonly binaries: Binaries;
  readonly layout: Layout;
  readonly engine: FauxEngine;
  readonly aether: Aetherd;
  readonly archon: Archond;
  /** Stops one daemon and starts it again on the same root, ports and configuration. */
  restart(daemon: "aether" | "archon"): Promise<void>;
  /** Stops both daemons and the engine, then deletes the root. Safe to call twice. */
  stop(): Promise<void>;
}

/** `AETHER_BIN` and `ARCHOND_BIN`, which scripts/e2e.sh resolves; both must be executables. Resolved
 * through symlinks, as /proc names them. */
export async function binariesFromEnv(env: NodeJS.ProcessEnv = process.env): Promise<Binaries> {
  const aether = env.AETHER_BIN;
  const archon = env.ARCHOND_BIN;
  if (!aether || !archon) throw new Error("AETHER_BIN and ARCHOND_BIN name the binaries under test; run the suite through scripts/e2e.sh");
  for (const binary of [aether, archon]) {
    assert.ok(path.isAbsolute(binary), `binary paths are absolute: ${binary}`);
    await access(binary, constants.X_OK);
  }
  return { aether: await realpath(aether), archon: await realpath(archon) };
}

export async function vendoredSchema(): Promise<ArchonSchema> {
  const schema = JSON.parse(await readFile(ARCHON_SCHEMA, "utf8")) as Partial<ArchonSchema>;
  assert.ok(typeof schema.protocol_version === "number" && Array.isArray(schema.features), `${ARCHON_SCHEMA} names protocol_version and features`);
  return { protocol_version: schema.protocol_version, features: schema.features };
}

/** What a stack runs: both daemons by default; `archon: false` is Aether alone, its listener with no mount. */
export interface StackOptions { readonly archon?: boolean }

async function boot(stack: Stack, options: StackOptions): Promise<void> {
  const { aether, archon, engine, layout } = stack;
  await aether.configure(providersToml(engine));
  await aether.start();
  if (options.archon === false) {
    await cli(aether.binary, ["--local", "remote", "enable"], layout);
    return;
  }
  await aether.wire();
  await archon.configure(aether.origin, layout.aether.socket);
  await archon.start();
  await archon.checkProtocol();
  await archon.wire();
}

export async function startStack(binaries: Binaries, options: StackOptions = {}): Promise<Stack> {
  const schema = await vendoredSchema();
  const ports = await freePorts();
  const layout = await createLayout();
  record("roots", layout.root);
  const engine = await startEngine().catch(async (error: unknown) => {
    await rm(layout.root, { recursive: true, force: true, maxRetries: 3 });
    throw error;
  });
  const aether = new Aetherd(binaries.aether, layout, ports);
  const archon = new Archond(binaries.archon, layout, ports, schema);
  let stopped = false;
  const stack: Stack = {
    binaries, layout, engine, aether, archon,
    async restart(daemon) {
      assert.ok(!stopped, "a stopped stack is not restarted");
      assert.ok(daemon === "aether" || options.archon !== false, "an Aether-only stack has no archond to restart");
      const target = daemon === "aether" ? aether : archon;
      await target.stop();
      await target.start();
    },
    async stop() {
      if (stopped) return;
      stopped = true;
      // Every step runs even when one fails, so one stuck daemon leaks nothing else.
      const steps = [() => archon.stop(), () => aether.stop(), () => engine.close(),
        () => rm(layout.root, { recursive: true, force: true, maxRetries: 3 })];
      const failures: unknown[] = [];
      for (const step of steps) await step().catch((error: unknown) => { failures.push(error); });
      if (failures.length > 0) throw new AggregateError(failures, `the stack at ${layout.root} did not stop cleanly`);
    },
  };
  try {
    await boot(stack, options);
  } catch (error) {
    await stack.stop();
    throw error;
  }
  return stack;
}
