// Setting up this computer, one step at a time as the app's setup page drives it: find the tools,
// connect Archon's tools to Aether once the user agrees natively, then start both. A restart of a
// running Aether happens only when that same confirmation named it.

import path from "node:path";
import type { SetupResult, SetupStep } from "@aether-ui/protocol";
import { publicDaemon } from "../bridge/bridge.ts";
import type { FoundDaemon } from "../discovery/daemons.ts";
import type { DaemonKind } from "../security/classes.ts";
import { confirmation, TEMPLATES, type Confirmation } from "../security/confirm.ts";
import { BridgeRefusal } from "../security/validate.ts";
import { ensureArchonConfig, parseArchonSetup } from "./archon-connect.ts";
import { firstLine, type CommandRunner, type ToolName } from "./commands.ts";
import { findTools } from "./tools.ts";

export interface SetupDeps {
  readonly run: CommandRunner;
  /** Archon's config file, where archon itself will read it. */
  readonly archonConfig: string;
  isReachable(kind: DaemonKind): Promise<boolean>;
  /** Turns on desktop.managed_start: the user's click on "Set up" is the consent. */
  enableManagedStart(): Promise<void>;
  confirm(owner: number, confirmation: Confirmation): Promise<boolean>;
  /** Managed start: returns at once when the daemon already answers. */
  start(kind: DaemonKind): Promise<void>;
  findDaemons(): Promise<FoundDaemon[]>;
  readonly stopDeadlineMs?: number;
  readonly pollMs?: number;
}

export const SETUP_STEPS: readonly SetupStep[] = ["tools", "connect", "start"];
/** Archon's own registration, which the app never reimplements. */
export const ARCHON_SETUP_ARGS: readonly string[] = ["setup", "--json"];
/** Aether's documented way to stop its daemon; agents resume when it starts again. Asked again, it
 * says whether the old daemon still holds its store, which a new one must wait for. */
export const AETHER_STOP_ARGS: readonly string[] = ["stop", "--json"];
const START_ORDER: readonly DaemonKind[] = ["aether", "archon"];
const NAMES: Readonly<Record<ToolName, string>> = { aether: "Aether", archon: "Archon" };

/** Aether came up after the dialog that did not name a restart; a retry asks with the restart named. */
const STARTED_MEANWHILE = "Aether started while setup was connecting Archon's tools, so it has not loaded them yet. Try again to restart it.";
const SETUP_TIMEOUT_MS = 60_000;
const STOP_TIMEOUT_MS = 30_000;
const STOP_DEADLINE_MS = 30_000;
const POLL_MS = 200;

function missingMessage(missing: readonly ToolName[]): string {
  if (missing.length === 0) throw new Error("a missing-tools message names a tool");
  return `Install ${missing.map((tool) => NAMES[tool]).join(" and ")} on this computer first, then try again.`;
}

/** Whether `aether stop --json` found a daemon still running or stopping; anything else it printed is a refusal. */
function stillStopping(stdout: string): boolean {
  let report: unknown;
  try {
    report = JSON.parse(stdout);
  } catch (error) {
    throw new BridgeRefusal(`Aether could not be restarted: its stop command answered unexpectedly (${String(error)}). Try again in a moment.`);
  }
  const stopping = report !== null && typeof report === "object" ? (report as { stopping?: unknown }).stopping : undefined;
  if (typeof stopping !== "boolean") throw new BridgeRefusal("Aether could not be restarted: its stop command answered unexpectedly. Try again in a moment.");
  return stopping;
}

export class Setup {
  private readonly deps: SetupDeps;
  /** Set only by a connect whose native confirmation named the restart. */
  private reloadOwed = false;
  private running = false;

  constructor(deps: SetupDeps) {
    if (!path.isAbsolute(deps.archonConfig)) throw new Error("Archon's config file is an absolute path");
    if (deps.pollMs !== undefined && !(deps.pollMs > 0)) throw new Error("setup polls at a positive interval");
    this.deps = deps;
  }

  /** Runs one step; a second call while one runs is refused, so two never overlap. */
  async run(owner: number, step: SetupStep): Promise<SetupResult> {
    if (!SETUP_STEPS.includes(step)) throw new Error(`not a setup step: ${String(step)}`);
    if (this.running) throw new BridgeRefusal("Setup is already running. Wait for it to finish, then try again.");
    this.running = true;
    try {
      switch (step) {
        case "tools": return { step, tools: await findTools(this.deps.run) };
        case "connect": return await this.connect(owner);
        case "start": return await this.start();
      }
    } finally {
      this.running = false;
    }
  }

  /** Asks natively before writing either config; a refusal leaves both untouched. */
  private async connect(owner: number): Promise<SetupResult> {
    this.reloadOwed = false;
    const missing = (await findTools(this.deps.run)).filter((tool) => !tool.found).map((tool) => tool.name);
    if (missing.length > 0) throw new BridgeRefusal(missingMessage(missing));
    const running = await this.deps.isReachable("aether");
    const template = TEMPLATES[running ? "setup.connect_restart" : "setup.connect"];
    if (!(await this.deps.confirm(owner, confirmation(template, {})))) throw new BridgeRefusal("Cancelled");
    ensureArchonConfig(this.deps.archonConfig);
    const result = await this.deps.run("archon", ARCHON_SETUP_ARGS, SETUP_TIMEOUT_MS);
    if (!result.found) throw new BridgeRefusal(missingMessage(["archon"]));
    if (result.code !== 0) {
      const detail = firstLine(result.stderr);
      throw new BridgeRefusal(`Setup could not connect Archon's tools to Aether.${detail === "" ? "" : ` Archon said: ${detail}`}`);
    }
    const answer = parseArchonSetup(result.stdout);
    // Aether reads its tool servers only when it starts, so a running one that lacks Archon's must restart.
    const unloaded = answer.mode === "shared" && answer.health === "fail" && await this.deps.isReachable("aether");
    if (unloaded && !running) throw new BridgeRefusal(STARTED_MEANWHILE);
    this.reloadOwed = unloaded;
    return { step: "connect", mode: answer.mode, registration: answer.registration, reload_owed: this.reloadOwed };
  }

  private async start(): Promise<SetupResult> {
    await this.deps.enableManagedStart();
    const restarted = this.reloadOwed && await this.deps.isReachable("aether") ? await this.restartAether() : false;
    this.reloadOwed = false;
    for (const kind of START_ORDER) await this.deps.start(kind);
    const daemons = (await this.deps.findDaemons()).map(publicDaemon);
    if (daemons.length === 0) throw new Error("discovery lists every daemon kind");
    return { step: "start", restarted, daemons };
  }

  /**
   * Stops Aether through its CLI, as the confirmed connect agreed, asking again until it reports no
   * daemon at all: a stopping one closes its socket before it lets go of its store.
   */
  private async restartAether(): Promise<true> {
    if (!this.reloadOwed) throw new Error("a restart runs only on the consent a confirmed connect recorded");
    const end = Date.now() + (this.deps.stopDeadlineMs ?? STOP_DEADLINE_MS);
    for (;;) {
      const stopped = await this.deps.run("aether", AETHER_STOP_ARGS, STOP_TIMEOUT_MS);
      if (!stopped.found) throw new BridgeRefusal(missingMessage(["aether"]));
      if (stopped.code !== 0) {
        const detail = firstLine(stopped.stderr);
        throw new BridgeRefusal(`Aether could not be restarted.${detail === "" ? "" : ` Aether said: ${detail}`} Try again in a moment.`);
      }
      if (!stillStopping(stopped.stdout)) return true;
      if (Date.now() >= end) throw new BridgeRefusal("Aether is taking a while to stop. Wait a moment, then try again.");
      await new Promise((resolve) => setTimeout(resolve, this.deps.pollMs ?? POLL_MS));
    }
  }
}
