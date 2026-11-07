// Setting up this computer, one step at a time as the app's setup page drives it: find the tools,
// connect Archon's tools to Aether, then start both, restarting Aether only after the user agrees.

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
/** Aether's documented way to stop its daemon; agents resume when it starts again. */
export const AETHER_STOP_ARGS: readonly string[] = ["stop"];
const START_ORDER: readonly DaemonKind[] = ["aether", "archon"];
const NAMES: Readonly<Record<ToolName, string>> = { aether: "Aether", archon: "Archon" };

const SETUP_TIMEOUT_MS = 60_000;
const STOP_TIMEOUT_MS = 30_000;
const STOP_DEADLINE_MS = 30_000;
const POLL_MS = 200;

function missingMessage(missing: readonly ToolName[]): string {
  if (missing.length === 0) throw new Error("a missing-tools message names a tool");
  return `Install ${missing.map((tool) => NAMES[tool]).join(" and ")} on this computer first, then try again.`;
}

export class Setup {
  private readonly deps: SetupDeps;
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
        case "connect": return await this.connect();
        case "start": return await this.start(owner);
      }
    } finally {
      this.running = false;
    }
  }

  private async connect(): Promise<SetupResult> {
    this.reloadOwed = false;
    const missing = (await findTools(this.deps.run)).filter((tool) => !tool.found).map((tool) => tool.name);
    if (missing.length > 0) throw new BridgeRefusal(missingMessage(missing));
    ensureArchonConfig(this.deps.archonConfig);
    const result = await this.deps.run("archon", ARCHON_SETUP_ARGS, SETUP_TIMEOUT_MS);
    if (!result.found) throw new BridgeRefusal(missingMessage(["archon"]));
    if (result.code !== 0) {
      const detail = firstLine(result.stderr);
      throw new BridgeRefusal(`Setup could not connect Archon's tools to Aether.${detail === "" ? "" : ` Archon said: ${detail}`}`);
    }
    const answer = parseArchonSetup(result.stdout);
    // Aether reads its tool servers only when it starts, so a running one that lacks Archon's must restart.
    this.reloadOwed = answer.mode === "shared" && answer.health === "fail" && await this.deps.isReachable("aether");
    return { step: "connect", mode: answer.mode, registration: answer.registration, reload_owed: this.reloadOwed };
  }

  private async start(owner: number): Promise<SetupResult> {
    await this.deps.enableManagedStart();
    const restarted = this.reloadOwed && await this.deps.isReachable("aether") ? await this.restartAether(owner) : false;
    this.reloadOwed = false;
    for (const kind of START_ORDER) await this.deps.start(kind);
    const daemons = (await this.deps.findDaemons()).map(publicDaemon);
    if (daemons.length === 0) throw new Error("discovery lists every daemon kind");
    return { step: "start", restarted, daemons };
  }

  /** Stops Aether through its CLI once the user agrees, and waits until it no longer answers. */
  private async restartAether(owner: number): Promise<true> {
    if (!(await this.deps.confirm(owner, confirmation(TEMPLATES["setup.restart"], {})))) throw new BridgeRefusal("Cancelled");
    const stopped = await this.deps.run("aether", AETHER_STOP_ARGS, STOP_TIMEOUT_MS);
    if (!stopped.found) throw new BridgeRefusal(missingMessage(["aether"]));
    if (stopped.code !== 0) {
      const detail = firstLine(stopped.stderr);
      throw new BridgeRefusal(`Aether could not be restarted.${detail === "" ? "" : ` Aether said: ${detail}`} Try again in a moment.`);
    }
    const end = Date.now() + (this.deps.stopDeadlineMs ?? STOP_DEADLINE_MS);
    while (await this.deps.isReachable("aether")) {
      if (Date.now() >= end) throw new BridgeRefusal("Aether is taking a while to stop. Wait a moment, then try again.");
      await new Promise((resolve) => setTimeout(resolve, this.deps.pollMs ?? POLL_MS));
    }
    return true;
  }
}
