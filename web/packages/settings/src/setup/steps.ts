// The steps of "Set up on this computer", each one call the wizard can run, show and retry:
// main does the host work (find, connect, start); the check reads the daemons through core.

import type { App } from "@aether-ui/core";
import type { aether, archon, ConnId, DesktopSetup, SetupResult, SetupStep, SetupTool } from "@aether-ui/protocol";
import { invariant } from "../kit/invariant.ts";
import { archonHealth, archonTools, providerReadiness, type Readiness, type ToolsCheck } from "./readiness.ts";

export type StepId = SetupStep | "check";

/** Every step in the order the wizard runs them. */
export const STEP_ORDER: readonly StepId[] = ["tools", "connect", "start", "check"];

/** Where each tool's installer is documented; an https link the app may open. */
export const INSTALL_GUIDE: Readonly<Record<SetupTool["name"], { readonly url: string; readonly command: string }>> = {
  aether: { url: "https://github.com/mengsig/aether#install", command: "curl -fsSL https://raw.githubusercontent.com/mengsig/aether/main/install.sh | sh" },
  archon: { url: "https://github.com/mengsig/archon#install", command: "curl -fsSL https://raw.githubusercontent.com/mengsig/archon/main/install.sh | sh" },
};

/** A step that could not finish, in words for the person at the screen, with what may fix it. */
export class SetupError extends Error {
  readonly missing: readonly SetupTool["name"][];
  readonly signIn: readonly ("claude" | "codex")[];
  /** The step a retry starts from, when it is an earlier one than the step that failed. */
  readonly retryFrom: StepId | undefined;

  constructor(message: string, help: { missing?: readonly SetupTool["name"][]; signIn?: readonly ("claude" | "codex")[]; retryFrom?: StepId } = {}) {
    super(message);
    this.name = "SetupError";
    this.missing = help.missing ?? [];
    this.signIn = help.signIn ?? [];
    this.retryFrom = help.retryFrom;
  }
}

export interface ProviderCheck { readonly id: string; readonly name: string; readonly readiness: Readiness }

/** What the steps learned; each step reads what earlier ones left and adds its own. */
export interface SetupFacts {
  readonly tools?: readonly SetupTool[];
  readonly mode?: "shared" | "dedicated";
  readonly registration?: "added" | "already_present" | "dedicated";
  readonly restarted?: boolean;
  readonly archonTools?: number;
  readonly providers?: readonly ProviderCheck[];
}

export interface StepDeps {
  readonly app: App;
  readonly setup: DesktopSetup;
  /** How long the check waits for the daemons to answer and Archon to see its tools. */
  readonly waitMs?: number;
  readonly sleep?: (ms: number) => Promise<void>;
}

const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
const TOOL_LABEL: Readonly<Record<SetupTool["name"], string>> = { aether: "Aether", archon: "Archon" };

/** The desktop's setup bridge, or null where this app cannot set up the computer (a browser, an old desktop). */
export function desktopSetup(app: App): DesktopSetup | null {
  const desktop = app.host.desktop as (typeof app.host.desktop & Partial<DesktopSetup>) | undefined;
  if (desktop === undefined || typeof desktop.setup !== "function") return null;
  const setup = desktop.setup.bind(desktop);
  return { setup };
}

/** Runs one step and returns the facts it adds; throws a SetupError the screen can show. */
export async function runStep(step: StepId, facts: SetupFacts, deps: StepDeps): Promise<SetupFacts> {
  invariant(STEP_ORDER.includes(step), `unknown setup step ${step}`);
  if (step === "check") return check(facts, deps);
  const result = await bridgeStep(deps.setup, step);
  if (result.step === "tools") return toolsFound(result.tools);
  if (result.step === "connect") return { mode: result.mode, registration: result.registration };
  invariant(result.step === "start", "the bridge answered the step it was asked");
  await deps.app.rediscover();
  return { restarted: result.restarted };
}

async function bridgeStep(setup: DesktopSetup, step: SetupStep): Promise<SetupResult> {
  let result: SetupResult;
  try {
    result = await setup.setup(step);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new SetupError(message || "This step did not finish.");
  }
  invariant(result !== null && typeof result === "object" && result.step === step, `the setup bridge answered ${step}`);
  return result;
}

function toolsFound(tools: readonly SetupTool[]): SetupFacts {
  invariant(tools.length > 0, "the tools step lists the tools it looked for");
  invariant(tools.every((tool) => tool.name in TOOL_LABEL), "the tools step names only aether and archon");
  const missing = tools.filter((tool) => !tool.found).map((tool) => tool.name);
  if (missing.length > 0) {
    const names = missing.map((name) => TOOL_LABEL[name]).join(" and ");
    throw new SetupError(`${names} ${missing.length > 1 ? "are" : "is"} not installed on this computer yet. Install ${missing.length > 1 ? "them" : "it"}, then check again.`, { missing });
  }
  return { tools };
}

async function check(facts: SetupFacts, deps: StepDeps): Promise<SetupFacts> {
  const sleep = deps.sleep ?? wait;
  const deadline = Date.now() + (deps.waitMs ?? 30_000);
  const aetherConn = await localConnection(deps.app, "aether", deadline, sleep);
  const archonConn = await localConnection(deps.app, "archon", deadline, sleep);
  if (facts.mode !== "dedicated") {
    const tools = archonTools(listOf<aether.McpServerHealth>(await deps.app.manager.send(aetherConn, { cmd: "mcp_servers", refresh: true }), "MCP servers"));
    if (tools.state !== "ready") throw new SetupError(`${tools.detail} Connect them again to fix this.`, { retryFrom: "connect" });
  }
  const tools = await archonView(deps.app, archonConn, deadline, sleep);
  if (tools.state !== "ready") throw new SetupError(`${tools.detail} Connect them again to fix this.`, { retryFrom: "connect" });
  const providers = await providerChecks(deps.app, aetherConn);
  if (!providers.some((p) => p.readiness.ready)) {
    const signIn = providers.flatMap((p) => (p.readiness.signIn ? [p.readiness.signIn] : []));
    throw new SetupError("No model provider is ready to run work yet. Sign in to one, then check again.", { signIn });
  }
  return { archonTools: tools.tools, providers };
}

/** Archon checks Aether on its own clock, so its view may lag Aether's: ask until it agrees or time runs out. */
async function archonView(app: App, conn: ConnId, deadline: number, sleep: (ms: number) => Promise<void>): Promise<ToolsCheck> {
  for (;;) {
    const status = recordOf<archon.DaemonStatus>(await app.manager.send(conn, { cmd: "status" }), "Archon's status");
    const view = archonHealth(status.mcp);
    if (view.state === "ready" || view.state === "failed" || Date.now() >= deadline) return view;
    await sleep(500);
  }
}

async function providerChecks(app: App, conn: ConnId): Promise<ProviderCheck[]> {
  const providers = listOf<aether.ProviderInfo>(await app.manager.send(conn, { cmd: "providers" }), "providers");
  const auths = listOf<aether.AuthInfo>(await app.manager.send(conn, { cmd: "auth_status" }), "sign-ins");
  return providers.map((p) => ({ id: p.provider, name: p.name, readiness: providerReadiness(p, auths.find((a) => a.provider === p.provider)) }));
}

/** The desktop's own connection to a local daemon once it is ready, waiting for it until the deadline. */
async function localConnection(app: App, daemon: "aether" | "archon", deadline: number, sleep: (ms: number) => Promise<void>): Promise<ConnId> {
  for (;;) {
    const local = app.manager.all().find((c) => c.endpoint.daemon === daemon && c.endpoint.transport.id === "ipc");
    if (local?.current.state === "ready") return local.id;
    if (Date.now() >= deadline) throw new SetupError(`${TOOL_LABEL[daemon]} started, but the app could not connect to it. Try again in a moment.`);
    await sleep(250);
  }
}

function dataOf(reply: Record<string, unknown>, label: string): unknown {
  invariant(typeof reply.reply === "string", `${label} came back as a reply`);
  return reply.data;
}

function listOf<T>(reply: Record<string, unknown>, label: string): T[] {
  const data = dataOf(reply, label);
  if (!Array.isArray(data)) throw new SetupError(`The app could not read the ${label} it got back.`);
  return data as T[];
}

function recordOf<T>(reply: Record<string, unknown>, label: string): T {
  const data = dataOf(reply, label);
  if (data === null || typeof data !== "object" || Array.isArray(data)) throw new SetupError(`The app could not read ${label}.`);
  return data as T;
}
