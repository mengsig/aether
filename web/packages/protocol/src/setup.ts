// Setting up this computer from the desktop app: an extension of the frozen DesktopBridge of
// CONTRACTS.md §2.4. Main exposes it as `window.desktop.setup`; a host without it has no setup.

import type { LocalDaemon } from "./app.ts";

/** One step of setting up this computer: find the programs, connect Archon's tools, start the daemons. */
export type SetupStep = "tools" | "connect" | "start";

/** A program the setup needs, as main found it on this computer. */
export interface SetupTool { name: "aether" | "archon"; found: boolean; version?: string }

export type SetupResult =
  | { step: "tools"; tools: SetupTool[] }
  /** `reload_owed`: the running Aether has not loaded Archon's tool server, so the start step restarts it. */
  | { step: "connect"; mode: "shared" | "dedicated"; registration: "added" | "already_present" | "dedicated"; reload_owed: boolean }
  | { step: "start"; restarted: boolean; daemons: LocalDaemon[] };

/** The setup half of the bridge; a refusal's message is plain language the app shows as it is. */
export interface DesktopSetup { setup(step: SetupStep): Promise<SetupResult> }
