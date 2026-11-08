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

/** Load archived agents on demand, without replacing fresher live updates in the store. */
export async function loadArchived(app: App, info: ConnectionInfo, isCurrent: () => boolean): Promise<void> {
  invariant(info.daemon === "aether" && info.state === "ready", "archived agents require a ready Aether connection");
  const agents: aether.Agent[] = [];
  if (!info.features.includes("agents_paging")) {
    const reply = await app.send(info.id, { cmd: "get_tree", include_archived: true });
    if (!isCurrent()) return;
    invariant(reply.reply === "tree" && reply.data !== null && typeof reply.data === "object", "get_tree returns a tree");
    const nodes = (reply.data as { nodes?: Record<string, { agent: unknown }> }).nodes;
    invariant(nodes !== undefined && typeof nodes === "object", "tree has nodes");
    agents.push(...archivedAgents(Object.values(nodes).map((node) => node.agent)));
  } else {
    let after: string | undefined;
    for (let page = 0; page < 10000; page += 1) {
      const reply = await app.send(info.id, { cmd: "list_agents_page", query: { include_archived: true }, limit: 500, ...(after ? { after } : {}) });
      if (!isCurrent()) return;
      invariant(reply.reply === "agents_page" && reply.data !== null && typeof reply.data === "object", "list_agents_page returns a page");
      const data = reply.data as { agents?: unknown; next_after?: unknown };
      agents.push(...archivedAgents(data.agents));
      if (data.next_after == null) break;
      invariant(typeof data.next_after === "string" && data.next_after !== after && page < 9999, "agent pages advance and terminate");
      after = data.next_after;
    }
  }
  if (!isCurrent()) return;
  app.store.updateAether(info.id, (state) => {
    if (agents.every((agent) => state.agents[agent.id] !== undefined)) return state;
    const merged = { ...state.agents };
    for (const agent of agents) if (!merged[agent.id]) merged[agent.id] = agent;
    return { ...state, agents: merged };
  });
}
