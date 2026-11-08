import { routeHref } from "@aether-ui/core";
import type { GNode, GraphLegendEntry, NodeStyle } from "@aether-ui/graph";
import { EmptyState } from "@aether-ui/ui";
import { statusLabel, type TaskNode } from "./model.ts";

export const TASK_LEGEND: readonly GraphLegendEntry[] = [
  { label: "Queued", status: "queued" }, { label: "Running", status: "running" },
  { label: "Needs you", status: "operator" }, { label: "Held", status: "blocked" },
  { label: "Waiting", status: "waiting" }, { label: "Succeeded", status: "succeeded" },
  { label: "Failed", status: "failed" }, { label: "Cancelled", status: "cancelled" },
  { label: "Dependency", tone: "muted", edge: "dashed" }, { label: "Stacked on", tone: "muted", edge: "dotted" },
  { label: "Initiative order", tone: "muted", edge: "solid" }, { label: "Critical path", tone: "accent", edge: "solid" },
];

export function taskNodeStyle(node: GNode): NodeStyle {
  if (node.id.startsWith("initiative:")) return { shape: "rounded-rect", icon: "▤" };
  if (node.id.startsWith("stage:")) return { shape: "diamond", icon: "◇" };
  return { shape: "rounded-rect", icon: "□" };
}

export function TaskDetails({ node }: { node: TaskNode }) {
  return <>
    <strong>{node.title}</strong>
    <p>{statusLabel(node)}</p>
    <p>Pipeline: {node.pipeline}</p>
    <p>Stage: {node.stage ?? "Not started"}</p>
    <a className="ui-link" href={routeHref("#/archon/items/:item", { item: node.id })}>Open item</a>
  </>;
}

export function DagEmpty() {
  return <EmptyState title="Nothing in the graph"
    action={<a className="ui-button" data-variant="primary" href="#/archon/queue">Queue an item</a>}>
    Queued items and their dependencies appear here. Queue an item.
  </EmptyState>;
}
