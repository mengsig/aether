import { visibleGraph } from "@aether-ui/graph";
import type { Layout, GraphModel, GNode, GEdge, archon } from "@aether-ui/protocol";
import type { ItemView } from "../issue/join.ts";
import { invariant } from "../issue/ref.ts";

export interface ObjectRow { readonly object: archon.GithubObject; readonly items: readonly ItemView[] }
export type OverviewFilters = { repo: string; kind: string; state: string };
export const DEFAULT_FILTERS: OverviewFilters = { repo: "", kind: "", state: "" };
const WEEK = 7 * 86_400_000;

export function objectId(ref: archon.GithubRef): string {
  invariant(ref.repo.split("/").length === 2, "GitHub repositories have an owner and name");
  invariant(Number.isSafeInteger(ref.number) && ref.number > 0, "a GitHub object has a positive number");
  return `${ref.host.toLowerCase()}/${ref.repo.toLowerCase()}#${ref.number}`;
}

export function objectHref(ref: archon.GithubRef): string {
  return `#/github/${[ref.host, ...ref.repo.split("/"), String(ref.number)].map(encodeURIComponent).join("/")}`;
}

export function filterRows(rows: readonly ObjectRow[], filters: OverviewFilters): ObjectRow[] {
  return rows.filter(({ object }) => (!filters.repo || object.ref.repo === filters.repo)
    && (!filters.kind || object.ref.kind === filters.kind) && (!filters.state || object.state === filters.state));
}

export function groupRows(rows: readonly ObjectRow[]): ReadonlyMap<string, readonly ObjectRow[]> {
  const groups = new Map<string, ObjectRow[]>();
  for (const row of rows) {
    const repo = row.object.ref.repo;
    const group = groups.get(repo) ?? [];
    group.push(row);
    groups.set(repo, group);
  }
  return groups;
}

export function relationshipGraph(rows: readonly ObjectRow[]): GraphModel {
  const nodes = new Map<string, GNode>();
  const edges = new Map<string, GEdge>();
  const connect = (from: string, to: string, kind: string) => edges.set(`${from}|${to}|${kind}`, { from, to, kind });
  const groups = [...groupRows(rows).keys()].map((repo) => ({ id: `group:${repo}`, label: repo }));
  for (const row of rows) {
    const object = row.object;
    const repoId = `repo:${object.ref.repo}`;
    const group = `group:${object.ref.repo}`;
    const id = objectId(object.ref);
    nodes.set(repoId, { id: repoId, label: object.ref.repo, group });
    nodes.set(id, { id, label: `#${object.ref.number} ${object.title}`, status: `${object.ref.kind === "issue" ? "issue" : "pr"}-${object.draft && object.state === "open" ? "draft" : object.state}`, group });
    connect(repoId, id, "contains");
    for (const item of row.items) {
      const itemId = `item:${item.id}`;
      nodes.set(itemId, { id: itemId, label: item.title, status: item.execution.status ?? "unknown", group });
      connect(id, itemId, item.relations.join(", ") || "linked");
      for (const use of item.agents) {
        const agentId = `agent:${use.agent}`;
        nodes.set(agentId, { id: agentId, label: use.agent, group });
        connect(itemId, agentId, "agent");
      }
    }
  }
  invariant(nodes.size >= groups.length, "each repository group has a node");
  invariant([...edges.values()].every((edge) => nodes.has(edge.from) && nodes.has(edge.to)), "every relationship has both ends");
  return { nodes: [...nodes.values()], edges: [...edges.values()], groups, collapsed: new Set() };
}

export interface WeekBucket { readonly label: string; open: number; closed: number; merged: number }

export function weeklyBuckets(rows: readonly ObjectRow[], now: number): WeekBucket[] {
  invariant(Number.isFinite(now), "weekly activity has a real end time");
  const today = new Date(now);
  const midnight = Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), today.getUTCDate());
  const monday = midnight - ((today.getUTCDay() + 6) % 7) * 86_400_000;
  const start = monday - 7 * WEEK;
  const buckets = Array.from({ length: 8 }, (_, i) => ({ label: new Date(start + i * WEEK).toISOString().slice(0, 10), open: 0, closed: 0, merged: 0 }));
  for (const { object } of rows) {
    const updated = Date.parse(object.updated_at);
    invariant(Number.isFinite(updated), "cached activity has a valid updated_at");
    if (updated > now) continue;
    const index = Math.floor((updated - start) / WEEK);
    const bucket = buckets[index];
    if (bucket) bucket[object.state]++;
  }
  return buckets;
}

export function itemsPerPr(rows: readonly ObjectRow[]): { label: string; value: number }[] {
  return rows.filter(({ object }) => object.ref.kind === "pull_request")
    .map(({ object, items }) => ({ label: `${object.ref.repo}#${object.ref.number}`, value: new Set(items.map((item) => item.id)).size }));
}

export function relationshipLayout(model: GraphModel): Layout {
  const graph = visibleGraph(model);
  const byGroup = new Map<string, GNode[]>();
  for (const node of model.nodes) {
    invariant(node.group !== undefined, "overview nodes belong to a repository group");
    const nodes = byGroup.get(node.group) ?? [];
    nodes.push(node); byGroup.set(node.group, nodes);
  }
  const positions = new Map<string, { x: number; y: number }>();
  let offset = 0;
  for (const nodes of byGroup.values()) {
    const counts = [0, 0, 0, 0];
    for (const node of nodes) {
      const column = node.id.startsWith("repo:") ? 0 : node.id.startsWith("item:") ? 2 : node.id.startsWith("agent:") ? 3 : 1;
      const row = counts[column] ?? 0;
      counts[column] = row + 1;
      positions.set(node.id, { x: column * 300, y: offset + row * 90 });
    }
    offset += (Math.max(...counts) + 1) * 90;
  }
  invariant(positions.size === model.nodes.length, "every graph node has a unique position");
  return { hash: graph.hash, positions };
}
