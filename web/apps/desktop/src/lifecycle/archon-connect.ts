// Connecting Archon's tools to this computer's Aether: Archon's config file, written only when
// there is none, and the answer of `archon setup --json`, which does the registration itself.

import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { absoluteHome, join, type Machine } from "../discovery/socket-paths.ts";
import { BridgeRefusal } from "../security/validate.ts";

export const ARCHON_CONFIG_TEXT = `# Written by the Aether desktop app when you set up this computer. You can edit it.
[aether]
mode = "shared"
`;

/**
 * Archon's config file as archon-core reads it: $ARCHON_CONFIG_DIR, else `dirs::config_dir`.
 * `machine` carries the launch environment, where a set ARCHON_CONFIG_DIR is already absolute.
 */
export function archonConfigFile(machine: Machine): string {
  const explicit = machine.env.ARCHON_CONFIG_DIR;
  if (explicit !== undefined) {
    if (!explicit.startsWith("/")) throw new Error(`ARCHON_CONFIG_DIR is absolute in the launch environment, not ${explicit}`);
    return join(explicit, "config.toml");
  }
  const home = absoluteHome(machine);
  if (machine.platform === "darwin") return join(home, "Library/Application Support/archon/config.toml");
  const xdg = machine.env.XDG_CONFIG_HOME;
  const base = xdg !== undefined && xdg.startsWith("/") ? xdg : join(home, ".config");
  return join(base, "archon/config.toml");
}

/** Writes Archon's config so it shares this computer's Aether, unless a config file is already there. */
export function ensureArchonConfig(file: string): "written" | "kept" {
  if (!path.isAbsolute(file) || path.basename(file) !== "config.toml") throw new Error(`not an Archon config file: ${file}`);
  const dir = path.dirname(file);
  const refusal = (error: unknown) => new BridgeRefusal(
    `Setup could not save Archon's settings in ${dir} (${(error as NodeJS.ErrnoException).code ?? "unknown error"}). Check that the folder is yours and try again.`,
    { cause: error });
  try {
    mkdirSync(dir, { recursive: true, mode: 0o700 });
  } catch (error) {
    throw refusal(error);
  }
  try {
    writeFileSync(file, ARCHON_CONFIG_TEXT, { flag: "wx", mode: 0o600 });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "EEXIST") return "kept";
    throw refusal(error);
  }
  return "written";
}

export interface ArchonSetup {
  readonly mode: "shared" | "dedicated";
  readonly registration: "added" | "already_present" | "dedicated";
  readonly health: "ok" | "warn" | "fail";
}

const UNREADABLE = "Setup could not connect Archon's tools: Archon gave an answer the app does not understand. Update Archon, then try again.";
const LEVELS = ["ok", "warn", "fail"] as const;

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}

/** The health level of `archon setup`'s own `mcp health` check, or undefined when it is malformed. */
function healthLevel(value: unknown): ArchonSetup["health"] | undefined {
  const health = record(value);
  if (health === undefined || health.check !== "mcp health" || typeof health.detail !== "string") return undefined;
  return LEVELS.find((level) => level === health.level);
}

/** The answer of `archon setup --json`, checked field by field; anything else is a refusal. */
export function parseArchonSetup(stdout: string): ArchonSetup {
  let parsed: unknown;
  try {
    parsed = JSON.parse(stdout);
  } catch (error) {
    throw new BridgeRefusal(UNREADABLE, { cause: error });
  }
  const answer = record(parsed);
  if (answer === undefined || typeof answer.config !== "string" || answer.config === "") throw new BridgeRefusal(UNREADABLE);
  const health = healthLevel(answer.mcp_health);
  if (health === undefined) throw new BridgeRefusal(UNREADABLE);
  if (answer.mode === "dedicated") return { mode: "dedicated", registration: "dedicated", health };
  if (answer.mode !== "shared") throw new BridgeRefusal(UNREADABLE);
  if (answer.registration !== "added" && answer.registration !== "already_present") throw new BridgeRefusal(UNREADABLE);
  return { mode: "shared", registration: answer.registration, health };
}
