import type { App } from "@aether-ui/core";
import type { aether, ConnectionInfo } from "@aether-ui/protocol";
import { invariant } from "./invariant.ts";

function archivedAgents(values: unknown): aether.Agent[] {
  invariant(Array.isArray(values), "agent listing contains an array");
  return values.map((value: unknown) => {
    invariant(value !== null && typeof value === "object" && typeof (value as aether.Agent).id === "string", "each agent has an id");
    return value as aether.Agent;
  }).filter((agent) => agent.archived);
}

export interface ArchivePage { readonly ids: readonly string[]; readonly next: string | null }

export async function loadArchived(app: App, info: ConnectionInfo, after: string | null, isCurrent: () => boolean): Promise<ArchivePage | null> {
  invariant(info.daemon === "aether" && info.state === "ready", "archived agents require a ready Aether connection");
  invariant(after === null || after.length > 0, "archive cursor is nonempty");
  const paging = info.features.includes("agents_paging");
  const reply = await app.send(info.id, paging
    ? { cmd: "list_agents_page", query: { include_archived: true }, limit: 500, ...(after === null ? {} : { after }) }
    : { cmd: "get_tree", include_archived: true });
  if (!isCurrent()) return null;
  invariant(reply.reply === (paging ? "agents_page" : "tree") && reply.data !== null && typeof reply.data === "object", "archive listing has the expected reply");
  const data = reply.data as { agents?: unknown; next_after?: unknown; nodes?: Record<string, { agent: unknown }> };
  invariant(paging || (data.nodes !== null && typeof data.nodes === "object"), "tree has nodes");
  const agents = archivedAgents(paging ? data.agents : Object.values(data.nodes ?? {}).map(node => node.agent));
  const next = paging ? data.next_after ?? null : null;
  invariant(next === null || (typeof next === "string" && next.length > 0 && next !== after), "archive cursor advances");
  app.store.updateAether(info.id, state => {
    if (agents.every(agent => state.agents[agent.id] !== undefined)) return state;
    const merged = { ...state.agents };
    for (const agent of agents) if (!merged[agent.id]) merged[agent.id] = agent;
    return { ...state, agents: merged };
  });
  return { ids: agents.map(agent => agent.id), next };
}
