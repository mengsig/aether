// The knowledge graph as a GraphModel: propositions are nodes, each chain an edge from every premise
// to its conclusion. A node's status is its mastery, so the canvas draws it with the `--mastery-*` token.

import type { aether, GEdge, GNode, GraphModel, Layout } from "@aether-ui/protocol";
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

export function knowledgeModel(graph: KnowledgeGraph, mastery: ReadonlyMap<string, Mastery>): GraphModel {
  const ids = new Set(graph.nodes.map((node) => node.id));
  invariant(ids.size === graph.nodes.length, "proposition ids are unique");
  const nodes: GNode[] = graph.nodes.map((node) => ({ id: node.id, label: node.name, status: `mastery-${mastery.get(node.id) ?? "unseen"}` }));
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

/**
 * The force layout keeps 28px dots about 42px apart; labelled boxes are 168×44. Stretching x by 4.5 and
 * y by 2.3 keeps any two such boxes apart: |dx| ≥ 37.3 or |dy| ≥ 19.1 always holds at that distance.
 */
const SPREAD = { x: 4.5, y: 2.3 } as const;

export function spreadForLabels(layout: Layout): Layout {
  const positions = new Map<string, { x: number; y: number }>();
  for (const [id, at] of layout.positions) {
    invariant(Number.isFinite(at.x) && Number.isFinite(at.y), `node ${id} has a finite position`);
    positions.set(id, { x: at.x * SPREAD.x, y: at.y * SPREAD.y });
  }
  invariant(positions.size === layout.positions.size, "every placed node stays placed");
  return { hash: `${layout.hash}:spread`, positions };
}
