// `#/archon/graph`: every item of core's stored task graph, layered and grouped by initiative, with
// the critical path and why items are held up. Core keeps the graph current from the change stream.

import { selectItems, selectTaskGraph, useSelector, type ArchonGraph, type ArchonItem } from "@aether-ui/core";
import { layeredLayout } from "@aether-ui/graph";
import type { ConnectionInfo } from "@aether-ui/protocol";
import { Button, EmptyState, Skeleton } from "@aether-ui/ui";
import { useCallback, useMemo, useState } from "react";
import { useArchonAccess, useArchonConnection } from "../queue/archon.ts";
import { invariant } from "../queue/model.ts";
import { NotConnected } from "../queue/QueueScreen.tsx";
import { DagCanvas } from "./DagCanvas.tsx";
import { DagOverview, NodeInspector } from "./DagPanel.tsx";
import type { LayoutFn } from "./layout.ts";
import { criticalPath, dagData, heldUp, initiativeProgress } from "./model.ts";
import "./dag.css";

export interface DagWorkspaceProps {
  readonly graph: ArchonGraph;
  /** Core's item summaries, whose titles are newer than the graph's. */
  readonly items: Readonly<Record<string, ArchonItem>>;
  readonly layout: LayoutFn;
}

/** The graph, its canvas and its panel; the selection is the only state it keeps. */
export function DagWorkspace({ graph, items, layout }: DagWorkspaceProps) {
  const [showCompleted, setShowCompleted] = useState(false);
  const allData = useMemo(() => dagData(graph, undefined, items), [graph, items]);
  const data = useMemo(() => showCompleted ? allData : dagData(graph, (node) => node.state !== "succeeded" && node.state !== "cancelled", items), [allData, graph, items, showCompleted]);
  const path = useMemo(() => criticalPath(data), [data]);
  const held = useMemo(() => heldUp(data), [data]);
  const progress = useMemo(() => initiativeProgress(allData, held, items), [allData, held, items]);
  const [picked, setPicked] = useState<string>();
  // A selected item that left the graph is no longer selected.
  const selected = picked !== undefined && data.byId.has(picked) ? picked : undefined;
  const onSelect = useCallback((id: string) => setPicked(id), []);
  const onPath = useMemo(() => new Set(path.nodes), [path]);
  if (allData.nodes.length === 0 && progress.length === 0) {
    return <EmptyState title="Nothing in the graph">The task graph shows open items and how they depend on each other. Enqueue an item to see it here.</EmptyState>;
  }
  const summary = `${data.nodes.length.toLocaleString("en")} items · ${data.edges.length.toLocaleString("en")} links · ${progress.length} initiatives`;
  return (
    <div className="dag">
      <header className="dag-header">
        <p className="dag-muted">{summary}{data.truncated ? ` · showing the first ${data.nodes.length.toLocaleString("en")}` : ""}</p>
        <Button variant="quiet" size="sm" aria-pressed={showCompleted} onClick={() => setShowCompleted((value) => !value)}>Show completed</Button>
        <Legend />
      </header>
      <div className="dag-body">
        {data.nodes.length === 0
          ? <EmptyState title="No open items in the graph">Show completed items to see their dependency history.</EmptyState>
          : <DagCanvas data={data} path={path} layout={layout} label="Task graph" selected={selected} onSelect={onSelect} />}
        <aside className="dag-panel" aria-label={selected === undefined ? "Graph overview" : "Selected item"}>
          {selected === undefined
            ? <DagOverview data={data} path={path} held={held} progress={progress} onSelect={onSelect} />
            : <NodeInspector key={selected} data={data} id={selected} onPath={onPath.has(selected)} onSelect={onSelect} onClose={() => setPicked(undefined)} />}
        </aside>
      </div>
    </div>
  );
}

function Legend() {
  return (
    <ul className="dag-legend" aria-label="Legend">
      <li><span className="dag-swatch" data-kind="path" aria-hidden="true" />Critical path</li>
      <li><span className="dag-swatch" data-kind="blocked" aria-hidden="true" />Held up</li>
      <li><span className="dag-swatch" data-kind="running" aria-hidden="true" />Running</li>
      <li><span className="dag-swatch" data-kind="operator" aria-hidden="true" />Needs you</li>
    </ul>
  );
}

function Graph({ info, layout }: { readonly info: ConnectionInfo; readonly layout: LayoutFn }) {
  const access = useArchonAccess(info);
  const graph = useSelector(useMemo(() => selectTaskGraph(access.conn), [access.conn]));
  const items = useSelector(useMemo(() => selectItems(access.conn), [access.conn]));
  if (!access.has("task_graph") && access.ready) {
    return <EmptyState title="No task graph on this Archon">This archond does not serve the task graph. Update it to 1.5 or later to see how items depend on each other.</EmptyState>;
  }
  if (graph === null) {
    return <div className="dag" aria-busy="true" aria-label="Loading the task graph"><Skeleton height="1.5rem" width="40%" /><Skeleton height="28rem" /></div>;
  }
  return <DagWorkspace graph={graph} items={items} layout={layout} />;
}

/** `layout` is for tests, which count layouts; the route renders the worker's layered layout. */
export default function DagScreen({ layout = layeredLayout }: { readonly layout?: LayoutFn }) {
  invariant(typeof layout === "function", "the DAG screen lays out with a function");
  const info = useArchonConnection();
  return info ? <Graph info={info} layout={layout} /> : <NotConnected />;
}
