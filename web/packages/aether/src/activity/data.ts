import type { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";
import { records } from "../memory/shared.tsx";

export function configuredModels(value: unknown): aether.ModelInfo[] {
  const models = records<aether.ModelInfo>(value);
  invariant(models.every(m => typeof m.configured === "boolean"), "model availability is explicit");
  invariant(models.every(m => m.model && m.model.provider.length > 0 && m.model.model.length > 0), "models have provider-qualified identities");
  return models.filter(m => m.configured);
}

export function workspaceValidation(value: unknown): aether.WorkspaceValidation | undefined {
  if (value === null || value === undefined) return undefined;
  invariant(typeof value === "object" && "valid" in value && typeof value.valid === "boolean", "workspace validation has a verdict");
  const validation = value as aether.WorkspaceValidation;
  invariant(!validation.valid || (typeof validation.path === "string" && validation.path.startsWith("/")), "valid workspace has an absolute canonical path");
  return validation;
}

/**
 * The network a new agent gets from this daemon. An explicit scope that leaves `network` out is
 * denied, which also withholds every MCP tool, so the Start form names the daemon's own default.
 */
export function networkDefault(value: unknown): aether.NetworkPolicy | undefined {
  if (value === null || value === undefined) return undefined;
  invariant(typeof value === "object" && "pid" in value, "the daemon's status names its process");
  const policy = (value as aether.DaemonStatus).network_default ?? "denied";
  invariant(policy === "allowed" || policy === "denied", `a known network policy, not ${String(policy)}`);
  return policy;
}

export function orderedEvents(events: readonly aether.Event[]): aether.Event[] {
  invariant(events.every(e => typeof e.id === "string" && e.id.length > 0), "events have durable identities");
  invariant(events.every(e => Number.isSafeInteger(e.seq) && e.seq > 0), "events have positive durable sequences");
  return [...new Map(events.map(e => [e.id, e])).values()].sort((a, b) => b.seq - a.seq);
}

export function defaultWorkspace(workspaces: readonly aether.Workspace[], home?: aether.PathCompletions): string {
  invariant(workspaces.every(w => typeof w.path === "string" && typeof w.exists === "boolean"), "workspaces report paths and existence");
  invariant(home === undefined || Array.isArray(home.entries), "home completion is a list");
  const recent = [...workspaces].filter(w => w.exists).sort((a, b) => (b.active_at ?? "").localeCompare(a.active_at ?? ""));
  return recent[0]?.path ?? home?.entries.find(p => p.is_dir)?.path ?? "";
}

export function defaultModel(models: readonly aether.ModelInfo[]): string {
  invariant(models.every(m => typeof m.configured === "boolean"), "model readiness is explicit");
  invariant(models.every(m => m.model.provider && m.model.model), "models have qualified identities");
  const ready = models.filter(m => m.configured);
  const model = ready.find(m => m.is_default) ?? ready[0];
  return model ? `${model.model.provider}/${model.model.model}` : "";
}

/** "agent.turn_completed" as "Agent turn completed"; a bare "custom" is "Custom event". */
export function kindLabel(type: string): string {
  invariant(type.length > 0, "an event kind is named");
  const words = type.split(/[._]/).filter(word => word.length > 0);
  invariant(words.length > 0, `an event kind has words: ${type}`);
  if (words.length === 1 && words[0] === "custom") return "Custom event";
  const [first, ...rest] = words as [string, ...string[]];
  return [first[0]?.toUpperCase() + first.slice(1), ...rest].join(" ");
}

/** The family an event kind belongs to, as the filter shows it: "agent" as "Agent". */
export function familyLabel(family: string): string {
  invariant(family.length > 0, "a family is named");
  return family[0]?.toUpperCase() + family.slice(1);
}

export const EFFORTS = [
  { value: "low", label: "Low" }, { value: "medium", label: "Medium" }, { value: "high", label: "High" },
  { value: "x_high", label: "Extra high" }, { value: "max", label: "Max" },
] as const;
export const DEFAULT_EFFORT = "medium";

export const modelKey = (m: aether.ModelInfo): string => `${m.model.provider}/${m.model.model}`;

/** The user's choice if it is still ready, else the default model, else the first. */
export function pickModel(choices: readonly aether.ModelInfo[], chosen: string): aether.ModelInfo | undefined {
  invariant(choices.every(m => m.configured), "only ready models are offered");
  invariant(new Set(choices.map(modelKey)).size === choices.length, "model identities are unique");
  return choices.find(m => modelKey(m) === chosen) ?? choices.find(m => m.is_default) ?? choices[0];
}

export interface ModelGroup { readonly provider: string; readonly models: readonly aether.ModelInfo[] }

export function modelGroups(choices: readonly aether.ModelInfo[]): ModelGroup[] {
  const groups = new Map<string, aether.ModelInfo[]>();
  for (const m of choices) groups.set(m.provider_name || m.model.provider, [...groups.get(m.provider_name || m.model.provider) ?? [], m]);
  invariant([...groups.values()].reduce((n, g) => n + g.length, 0) === choices.length, "every model lands in one group");
  return [...groups].map(([provider, models]) => ({ provider, models }));
}

/** Existing workspaces, most recently active first. */
export function recentWorkspaces(value: unknown): aether.Workspace[] {
  const all = records<aether.Workspace>(value);
  invariant(all.every(w => typeof w.path === "string" && w.path.startsWith("/")), "workspaces have absolute paths");
  invariant(all.every(w => typeof w.exists === "boolean"), "workspace existence is explicit");
  return all.filter(w => w.exists).sort((a, b) => (b.active_at ?? "").localeCompare(a.active_at ?? ""));
}

/** The folder's own name and the path leading to it: `/a/b/repo` is `repo` in `/a/b`. */
export function splitPath(path: string): { name: string; parent: string } {
  invariant(path.startsWith("/"), "a workspace path is absolute");
  const trimmed = path.length > 1 ? path.replace(/\/+$/, "") : path;
  const cut = trimmed.lastIndexOf("/");
  const name = trimmed.slice(cut + 1);
  invariant(name.length > 0 || trimmed === "/", "a folder path ends in a name");
  return { name: name || "/", parent: cut <= 0 ? "" : trimmed.slice(0, cut) };
}

/** The agent a `create_agent` reply describes. */
export function createdAgent(reply: Record<string, unknown>): { id: string; name: string } {
  invariant(reply.reply === "agent", `create_agent answers with an agent, not ${String(reply.reply)}`);
  const data = reply.data as { id?: unknown; name?: unknown } | null;
  invariant(typeof data?.id === "string" && data.id.length > 0, "the created agent has an id");
  return { id: data.id, name: typeof data.name === "string" && data.name !== "" ? data.name : data.id };
}
