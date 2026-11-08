// The real engines for the on-demand journey variant: claude-code and codex through the user's
// own CLI logins, on a stack whose daemon state stays in its temp root. Spend is capped by the
// daemon's run stops, by a per-agent cost budget and turn limit, and by two cheap models only.

import assert from "node:assert/strict";
import { access, chmod, constants, mkdir, realpath, symlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { userHome } from "./env.ts";
import { until } from "./process.ts";
import type { Stack } from "./stack.ts";

/** The only models the real variant may run, cheapest of each engine. */
export const REAL_MODELS = [
  { provider: "claude-code", model: "claude-haiku-5-5", cli: "claude" },
  { provider: "codex", model: "gpt-6-luna", cli: "codex" },
] as const;
export const REAL_DEFAULT = `${REAL_MODELS[0].provider}/${REAL_MODELS[0].model}`;
/** The daemon interrupts a run once its recorded spend reaches this (`run_hard_stop_usd`). */
export const RUN_HARD_STOP_USD = 0.5;
/** New delegation is refused once a run has spent this (`run_budget_usd`). */
export const RUN_BUDGET_USD = 1;
/** For agents the harness creates itself: spend and turn caps (`AgentConfig`). */
export const AGENT_LIMITS = { cost_budget: 0.25, max_turns: 4 } as const;
const READY_MS = 120_000;
const SYSTEM_PATH = ["/usr/local/bin", "/usr/bin", "/bin"];
/** What the engines' network-denied sandboxes run on Linux; linked from the user's PATH when there. */
const SANDBOX_TOOLS = ["bwrap", "socat"] as const;

export function isRealModel(model: { provider: string; model: string }): boolean {
  return REAL_MODELS.some((real) => real.provider === model.provider && real.model === model.model);
}

/** `config.toml` for the real variant: Haiku by default, one engine turn at a time, the run stops. */
export function realConfigToml(port: number): string {
  assert.ok(Number.isInteger(port) && port > 0, `the listener port is a port: ${port}`);
  return [
    "[model]", `default = "${REAL_DEFAULT}"`, "",
    "[scheduler]", "max_concurrent_engine_turns = 1", `run_budget_usd = ${RUN_BUDGET_USD.toFixed(2)}`, `run_hard_stop_usd = ${RUN_HARD_STOP_USD.toFixed(2)}`, "",
    "[remote]", `address = "127.0.0.1:${port}"`, "",
  ].join("\n");
}

/**
 * Links the programs a network-denied turn's sandbox needs (`bwrap`, `socat`) beside the CLIs,
 * from wherever the user's PATH has them. One that is missing stays missing: the engine then says so.
 */
async function linkSandboxTools(engines: string, searchPath: string): Promise<void> {
  assert.ok(path.isAbsolute(engines), `the engines dir is absolute: ${engines}`);
  for (const tool of SANDBOX_TOOLS) {
    const found = await findCli(tool, searchPath).then((file) => file, () => null);
    if (found !== null) await symlink(found, path.join(engines, tool));
  }
}

/** The first executable named `name` on `searchPath`, resolved through symlinks. */
async function findCli(name: string, searchPath: string): Promise<string> {
  for (const dir of searchPath.split(":").filter((entry) => path.isAbsolute(entry))) {
    const candidate = path.join(dir, name);
    if (await access(candidate, constants.X_OK).then(() => true, () => false)) return realpath(candidate);
  }
  throw new Error(`the real variant needs the ${name} CLI on PATH, logged in; none on ${searchPath}`);
}

/** Where each CLI keeps its login; overridable, else the CLI's own default under the real home. */
function loginDirs(env: NodeJS.ProcessEnv): { claude: string; codex: string } {
  const home = userHome();
  const claude = env.E2E_CLAUDE_CONFIG_DIR ?? path.join(home, ".claude");
  const codex = env.E2E_CODEX_HOME ?? path.join(home, ".codex");
  for (const dir of [claude, codex]) assert.ok(path.isAbsolute(dir), `a login directory is absolute: ${dir}`);
  return { claude, codex };
}

/**
 * The daemon environment for real engines: the stack's own, with the two real CLIs linked ahead
 * of the stubs and pointed at their login directories. Nothing is copied; daemon dirs stay put.
 */
export async function realEngineEnv(stack: Stack, env: NodeJS.ProcessEnv = process.env): Promise<NodeJS.ProcessEnv> {
  const { layout } = stack;
  const engines = path.join(layout.root, "engines");
  await mkdir(engines, { mode: 0o700 });
  const searchPath = env.E2E_USER_PATH ?? env.PATH ?? "";
  for (const { cli } of REAL_MODELS) await symlink(await findCli(cli, searchPath), path.join(engines, cli));
  await linkSandboxTools(engines, searchPath);
  const logins = loginDirs(env);
  // The CLIs write caches under HOME; the stack's empty home is read-only by default.
  await chmod(layout.home, 0o700);
  const real: NodeJS.ProcessEnv = { ...layout.env, PATH: [engines, layout.bin, ...SYSTEM_PATH].join(":"), CLAUDE_CONFIG_DIR: logins.claude, CODEX_HOME: logins.codex };
  for (const name of ["AETHER_CONFIG_DIR", "AETHER_DATA_DIR", "AETHER_SOCKET", "ARCHON_DATA_DIR", "HOME"] as const) {
    assert.ok(String(real[name]).startsWith(`${layout.root}/`), `${name} stays inside the stack root`);
  }
  return real;
}

/**
 * Turns a faux stack's aetherd onto the real engines: the same root and sockets, its env and
 * config.toml replaced, then a restart. The faux provider stays, so archond's orchestrator runs free.
 */
export async function enableRealEngines(stack: Stack): Promise<void> {
  const env = await realEngineEnv(stack);
  // Layout.env is the one object every daemon start and CLI call reads; env is a superset of it.
  Object.assign(stack.layout.env, env);
  assert.equal(stack.layout.env.PATH, env.PATH, "the daemons now find the real CLIs first");
  await writeFile(path.join(stack.layout.aether.config, "config.toml"), realConfigToml(stack.aether.ports.aether));
  await stack.restart("aether");
}

export interface Readiness {
  readonly ready: boolean;
  /** `provider/model` of each real model `list_models` does not show as configured. */
  readonly missing: readonly string[];
}

async function configuredModels(stack: Stack): Promise<string[]> {
  const { reply, data } = await stack.aether.request({ cmd: "list_models" });
  assert.equal(reply, "models", `list_models answers models: ${reply}`);
  const models = (Array.isArray(data) ? data : (data as { models?: unknown }).models) as Array<{ model?: { provider?: string; model?: string }; configured?: boolean }>;
  assert.ok(Array.isArray(models), `list_models lists models: ${JSON.stringify(data).slice(0, 400)}`);
  return models.filter((info) => info.configured === true).map((info) => `${info.model?.provider}/${info.model?.model}`);
}

/** Both real models configured on the stack's aetherd, waiting out its engine probes; else what is missing. */
export async function checkReadiness(stack: Stack, timeoutMs = READY_MS): Promise<Readiness> {
  const wanted = REAL_MODELS.map((real) => `${real.provider}/${real.model}`);
  const missingOf = (configured: readonly string[]) => wanted.filter((model) => !configured.includes(model));
  const configured = await until(() => configuredModels(stack), (models) => missingOf(models).length === 0, "both real models to be configured", timeoutMs)
    .catch(async (error: unknown) => {
      if (!(error instanceof Error && error.message.startsWith("Timed out"))) throw error;
      return configuredModels(stack);
    });
  const missing = missingOf(configured);
  return { ready: missing.length === 0, missing };
}

/** What the real variant tells a reader when it skips. */
export function readinessMessage(readiness: Readiness): string {
  if (readiness.ready) return "both real engines are ready";
  return `the real engines are not ready: ${readiness.missing.join(", ")} not configured. Log in with \`claude\` and \`codex\` `
    + "(or point E2E_CLAUDE_CONFIG_DIR / E2E_CODEX_HOME at their login dirs) and run again";
}
