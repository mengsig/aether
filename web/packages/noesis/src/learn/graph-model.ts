// The knowledge graph as a GraphModel: propositions are nodes, each chain an edge from every premise
// to its conclusion, titled for reading. A node's status is its mastery, drawn with the `--mastery-*` token.

import type { aether, GEdge, GNode, GraphModel } from "@aether-ui/protocol";
import { invariant, isMastery, type Mastery } from "./model.ts";

export interface KnowledgeGraph {
  readonly nodes: readonly aether.Proposition[];
  readonly chains: readonly aether.Chain[];
  readonly truncated: boolean;
}

/** The `noesis_graph` reply, checked at the boundary. */
export function knowledgeGraph(value: Record<string, unknown> | undefined): KnowledgeGraph | undefined {
  if (value === undefined) return undefined;
  invariant(Array.isArray(value.nodes) && Array.isArray(value.chains), "noesis_graph carries nodes and chains");
  return { nodes: value.nodes as aether.Proposition[], chains: value.chains as aether.Chain[], truncated: value.truncated === true };
}

/** Mastery by proposition id, from the learner's concepts that name a proposition. */
export function masteryByProposition(concepts: readonly aether.ConceptState[]): Map<string, Mastery> {
  const map = new Map<string, Mastery>();
  for (const concept of concepts) {
    if (concept.concept.kind !== "proposition") continue;
    invariant(isMastery(concept.state), "a concept has a known state");
    map.set(concept.concept.ref, concept.state);
  }
  return map;
}

const TITLE_MAX = 60;

/** `raft-leader_election` reads "Raft leader election". */
function humanize(slug: string): string {
  const words = slug.split(/[-_]+/).filter((word) => word !== "").join(" ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** The statement up to its first full stop, semicolon or colon, cut to a title's length. */
function firstClause(statement: string): string {
  const clause = statement.trim().split(/[.;:](?:\s|$)/, 1)[0]?.trim() ?? "";
  return clause.length > TITLE_MAX ? `${clause.slice(0, TITLE_MAX - 1).trimEnd()}…` : clause;
}

/** A proposition as a person reads it: a slug name humanized, else its name, else its statement's first clause. */
export function readableTitle(proposition: Pick<aether.Proposition, "name" | "statement">): string {
  const name = proposition.name.trim();
  const title = /[-_]/.test(name) && !/\s/.test(name) ? humanize(name) : name !== "" ? name : firstClause(proposition.statement);
  invariant(title !== "", "a proposition has a name or a statement to title it");
  invariant(title.length <= Math.max(TITLE_MAX, name.length), "a title is no longer than its source");
  return title;
}

export function knowledgeModel(graph: KnowledgeGraph, mastery: ReadonlyMap<string, Mastery>): GraphModel {
  const ids = new Set(graph.nodes.map((node) => node.id));
  invariant(ids.size === graph.nodes.length, "proposition ids are unique");
  const nodes: GNode[] = graph.nodes.map((node) => ({ id: node.id, label: readableTitle(node), status: `mastery-${mastery.get(node.id) ?? "unseen"}` }));
  const seen = new Set<string>();
  const edges: GEdge[] = [];
  for (const chain of graph.chains) {
    // A truncated graph may name endpoints it did not send; those edges are left out.
    if (!ids.has(chain.conclusion)) continue;
    for (const premise of chain.premises) {
      const key = `${premise}>${chain.conclusion}`;
      if (!ids.has(premise) || premise === chain.conclusion || seen.has(key)) continue;
      seen.add(key);
      edges.push({ from: premise, to: chain.conclusion, kind: "entails" });
    }
  }
  invariant(edges.every((edge) => ids.has(edge.from) && ids.has(edge.to)), "every edge joins two drawn propositions");
  return { nodes, edges, groups: [], collapsed: new Set() };
}

/** Changes only when propositions or chains come or go, so a mastery change never relayouts. */
export function structureKey(model: GraphModel): string {
  return `${model.nodes.map((node) => node.id).join(",")}|${model.edges.map((edge) => `${edge.from}>${edge.to}`).join(",")}`;
}
