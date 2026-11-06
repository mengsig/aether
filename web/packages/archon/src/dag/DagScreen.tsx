// `#/archon/graph`: every item of core's stored task graph, layered and grouped by initiative, with
// the critical path and why items are held up. Core keeps the graph current from the change stream.

import { selectItems, selectTaskGraph, useSelector, type ArchonGraph, type ArchonItem } from "@aether-ui/core";
import { layeredLayout } from "@aether-ui/graph";
import type { ConnectionInfo } from "@aether-ui/protocol";
import { EmptyState, Skeleton } from "@aether-ui/ui";
import { useCallback, useMemo, useState } from "react";
import { useArchonAccess, useArchonConnection } from "../queue/archon.ts";
import { invariant } from "../queue/model.ts";
import { NotConnected } from "../queue/QueueScreen.tsx";
import { DagCanvas } from "./DagCanvas.tsx";
import { DagOverview, NodeInspector } from "./DagPanel.tsx";
import type { LayoutFn } from "./layout.ts";
import { criticalPath, dagData, heldUp, initiativeProgress } from "./model.ts";
import { DagEmpty } from "./presentation.tsx";
import "./dag.css";

export interface DagWorkspaceProps {
  readonly graph: ArchonGraph;
  /** Core's item summaries, whose titles are newer than the graph's. */
  readonly items: Readonly<Record<string, ArchonItem>>;
  readonly layout: LayoutFn;
}

/** The graph, its canvas and its panel; the selection is the only state it keeps. */
export function DagWorkspace({ graph, items, layout }: DagWorkspaceProps) {
  const data = useMemo(() => dagData(graph, undefined, items), [graph, items]);
  const path = useMemo(() => criticalPath(data), [data]);
  const held = useMemo(() => heldUp(data), [data]);
  const progress = useMemo(() => initiativeProgress(data, held), [data, held]);
  const [picked, setPicked] = useState<string>();
  // A selected item that left the graph is no longer selected.
  const selected = picked !== undefined && data.byId.has(picked) ? picked : undefined;
  const onSelect = useCallback((id: string) => setPicked(id), []);
  const onPath = useMemo(() => new Set(path.nodes), [path]);
  if (data.nodes.length === 0) {
    return <DagEmpty />;
  }
  const summary = `${data.nodes.length.toLocaleString("en")} items · ${data.edges.length.toLocaleString("en")} links · ${progress.length} initiatives`;
  return (
    <div className="dag">
      <header className="dag-header">
        <p className="dag-muted">{summary}{data.truncated ? ` · showing the first ${data.nodes.length.toLocaleString("en")}` : ""}</p>
        <p className="dag-muted">Select an item to inspect · drag to pan · scroll to zoom</p>
      </header>
      <div className="dag-body">
        <DagCanvas data={data} path={path} layout={layout} label="Task graph" selected={selected} onSelect={onSelect} />
        <aside className="dag-panel" aria-label={selected === undefined ? "Graph overview" : "Selected item"}>
          {selected === undefined
            ? <DagOverview data={data} path={path} held={held} progress={progress} onSelect={onSelect} />
            : <NodeInspector key={selected} data={data} id={selected} onPath={onPath.has(selected)} onSelect={onSelect} onClose={() => setPicked(undefined)} />}
        </aside>
      </div>
    </div>
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
