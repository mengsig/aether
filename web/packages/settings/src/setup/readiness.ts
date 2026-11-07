// Plain-language readiness for what the app needs on this computer: a model provider that can run
// work now, and Archon's tools loaded into Aether. Pure, so the wizard and Settings agree.

import type { aether, archon } from "@aether-ui/protocol";
import type { Tone } from "@aether-ui/ui";
import { invariant } from "../kit/invariant.ts";

/** The MCP server name `archon setup` registers in Aether's config. */
export const ARCHON_SERVER = "archon";

/** The engine sign-in each provider id uses. */
export const ENGINE_OF: Readonly<Record<string, "claude" | "codex">> = { "claude-code": "claude", codex: "codex" };

export interface Readiness {
  readonly ready: boolean;
  readonly tone: Tone;
  readonly text: string;
  /** What to do about it, in words a person acts on; absent when nothing is needed. */
  readonly fix?: string;
  /** The engine whose sign-in fixes it, when signing in is the fix. */
  readonly signIn?: "claude" | "codex";
}

/** Whether a provider can run work now, folding configured, enabled, sign-in and health into one answer. */
export function providerReadiness(provider: aether.ProviderInfo, auth: aether.AuthInfo | undefined): Readiness {
  invariant(provider.provider !== "", "a provider has an id");
  invariant(auth === undefined || auth.provider === provider.provider, "the sign-in belongs to this provider");
  const engine = ENGINE_OF[provider.provider];
  const signIn = provider.engine && engine !== undefined ? { signIn: engine } : {};
  if (!provider.enabled) return { ready: false, tone: "neutral", text: "Turned off", fix: "Turn it on in Aether's settings to use it." };
  const signedIn = provider.configured && (auth === undefined || auth.configured);
  if (!signedIn) {
    const fix = signIn.signIn ? `Sign in to ${provider.name} to use its models.` : `Add a key for ${provider.name} in Aether's settings to use it.`;
    return { ready: false, tone: "warning", text: signIn.signIn ? "Not signed in" : "Not set up", fix, ...signIn };
  }
  const health = provider.health;
  if (health.backoff) return { ready: false, tone: "warning", text: "Paused for now", fix: `It recovers on its own: ${health.backoff.reason}.` };
  if ((health.suspended_agents ?? 0) > 0) return { ready: true, tone: "warning", text: "Ready, some work paused" };
  return { ready: true, tone: "success", text: health.running > 0 ? `Ready, ${health.running} running` : "Ready" };
}

export type ToolsCheck =
  | { readonly state: "ready"; readonly tools: number }
  | { readonly state: "missing" | "starting" | "failed"; readonly detail: string };

const MCP_STATES: ReadonlySet<unknown> = new Set(["unknown", "ready", "failed"]);

/** A `mcp_servers` reply checked entry by entry: a name, a known state and, if any, a tool list; else null. */
export function mcpServersOf(value: unknown): readonly aether.McpServerHealth[] | null {
  if (!Array.isArray(value)) return null;
  const valid = value.every((entry: unknown) => {
    if (entry === null || typeof entry !== "object" || Array.isArray(entry)) return false;
    const server = entry as Record<string, unknown>;
    const tools = server.tools;
    return typeof server.name === "string" && server.name !== "" && MCP_STATES.has(server.state) && (tools === undefined || tools === null || Array.isArray(tools));
  });
  return valid ? (value as aether.McpServerHealth[]) : null;
}

/** Archon's tools as Aether sees them, from a `mcp_servers` reply. */
export function archonTools(servers: readonly aether.McpServerHealth[]): ToolsCheck {
  invariant(Array.isArray(servers), "mcp_servers answers a list");
  const server = servers.find((s) => s.name === ARCHON_SERVER);
  if (server === undefined) return { state: "missing", detail: "Archon's tools are not connected to Aether yet." };
  if (server.state === "failed") return { state: "failed", detail: server.error ? `Aether could not reach Archon's tools: ${server.error}` : "Aether could not reach Archon's tools." };
  if (server.state === "unknown") return { state: "starting", detail: "Aether has not contacted Archon's tools yet." };
  const tools = server.tools?.length ?? 0;
  invariant(tools >= 0, "a tool count is never negative");
  if (tools === 0) return { state: "failed", detail: "Archon's tools answered, but offered no tools." };
  return { state: "ready", tools };
}

/** Archon's own view of its tools in Aether, from `status.mcp.health`. */
export function archonHealth(mcp: archon.McpStatus | null | undefined): ToolsCheck {
  if (mcp === null || mcp === undefined) return { state: "starting", detail: "Archon has not checked its tools in Aether yet." };
  const health = mcp.health;
  invariant(typeof health.state === "string", "an MCP health names its state");
  switch (health.state) {
    case "ready": return health.tools > 0 ? { state: "ready", tools: health.tools } : { state: "failed", detail: "Archon's tools offered no tools." };
    case "not_loaded": return { state: "missing", detail: "Aether has not loaded Archon's tools." };
    case "unknown": return { state: "starting", detail: "Aether has not contacted Archon's tools yet." };
    case "failed": return { state: "failed", detail: `Aether could not reach Archon's tools: ${health.error}` };
    case "unreachable": return { state: "failed", detail: `Archon could not reach Aether: ${health.error}` };
  }
}
