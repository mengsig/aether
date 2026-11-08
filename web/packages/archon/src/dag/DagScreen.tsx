// `#/archon/graph`: every item of core's stored task graph, layered and grouped by initiative, with
// the critical path and why items are held up. Core keeps the graph current from the change stream.

import { selectItems, selectTaskGraph, useApp, useSelector, type ArchonGraph, type ArchonItem, type Stamped, type State } from "@aether-ui/core";
import { layeredLayout } from "@aether-ui/graph";
import type { archon, ConnectionInfo } from "@aether-ui/protocol";
import { Button, EmptyState, Skeleton } from "@aether-ui/ui";
import { useCallback, useMemo, useState } from "react";
import { useArchonAccess, useArchonConnection, useSettled } from "../queue/archon.ts";
import { useStoreRead } from "../initiatives/read.ts";
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
  readonly progressItems?: Readonly<Record<string, ArchonItem>> | null;
  readonly progressFailure?: string | null;
  readonly retryProgress?: () => void;
  readonly initialShowCompleted?: boolean;
}

/** The graph, its canvas and its panel; the selection is the only state it keeps. */
export function DagWorkspace({ graph, items, layout, progressItems = items, progressFailure = null, retryProgress, initialShowCompleted = false }: DagWorkspaceProps) {
  const [showCompleted, setShowCompleted] = useState(initialShowCompleted);
  const allData = useMemo(() => dagData(graph, undefined, items), [graph, items]);
  const data = useMemo(() => showCompleted ? allData : dagData(graph, (node) => node.state !== "succeeded" && node.state !== "cancelled", items), [allData, graph, items, showCompleted]);
  const path = useMemo(() => criticalPath(data), [data]);
  const held = useMemo(() => heldUp(data), [data]);
  const progress = useMemo(() => initiativeProgress(allData, held, progressItems ?? {}), [allData, held, progressItems]);
  const [picked, setPicked] = useState<string>();
  // A selected item that left the graph is no longer selected.
  const selected = picked !== undefined && data.byId.has(picked) ? picked : undefined;
  const onSelect = useCallback((id: string) => setPicked(id), []);
  const onPath = useMemo(() => new Set(path.nodes), [path]);
  if (allData.nodes.length === 0 && progress.length === 0) {
    return <DagEmpty />;
  }
  const initiativeCount = progressItems === null ? data.initiatives.length : progress.length;
  const summary = `${data.nodes.length.toLocaleString("en")} items · ${data.edges.length.toLocaleString("en")} links · ${initiativeCount} initiatives`;
  return (
    <div className="dag">
      <header className="dag-header">
        <p className="dag-muted">{summary}{data.truncated ? ` · showing the first ${data.nodes.length.toLocaleString("en")}` : ""}</p>
        <Button variant="quiet" size="sm" aria-pressed={showCompleted} onClick={() => setShowCompleted((value) => !value)}>Show completed</Button>
        <Legend />
        <p className="dag-muted">Select an item to inspect · drag to pan · scroll to zoom</p>
      </header>
      <div className="dag-body">
        {data.nodes.length === 0
          ? <EmptyState title="No open items in the graph">Show completed items to see their dependency history.</EmptyState>
          : <DagCanvas data={data} path={path} layout={layout} label="Task graph" selected={selected} onSelect={onSelect} />}
        <aside className="dag-panel" aria-label={selected === undefined ? "Graph overview" : "Selected item"}>
          {selected === undefined
            ? <DagOverview data={data} path={path} held={held} progress={progress} onSelect={onSelect} />
            : <NodeInspector key={selected} data={data} id={selected} onPath={onPath.has(selected)} onSelect={onSelect} onClose={() => setPicked(undefined)} />}
          {selected === undefined && progressItems === null ? <p className="dag-muted">{progressFailure ?? "Loading initiative progress…"}{progressFailure && retryProgress ? <Button variant="quiet" size="sm" onClick={retryProgress}>Try again</Button> : null}</p> : null}
        </aside>
      </div>
    </div>
  );
}

const NO_DETAILS: Readonly<Record<string, Stamped>> = {};

function progressMembers(ids: readonly string[], details: Readonly<Record<string, Stamped>>, items: Readonly<Record<string, ArchonItem>>): Record<string, ArchonItem> | null {
  const result: Record<string, ArchonItem> = {};
  for (const id of ids) {
    const stamped = details[id];
    if (!stamped) return null;
    const detail = stamped.value as unknown as archon.InitiativeDetail;
    invariant(detail.initiative.id === id && Array.isArray(detail.members), "initiative progress reads its members");
    for (const member of detail.members) {
      const live = items[member.id];
      result[member.id] = live && live.asOf !== null && (stamped.asOf === null || live.asOf > stamped.asOf) ? live
        : { summary: member as unknown as Record<string, unknown>, state: member.state.state, asOf: stamped.asOf };
    }
  }
  return result;
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
  const app = useApp();
  const access = useArchonAccess(info);
  const graph = useSelector(useMemo(() => selectTaskGraph(access.conn), [access.conn]));
  const items = useSelector(useMemo(() => selectItems(access.conn), [access.conn]));
  const ids = useMemo(() => graph?.initiatives.map((initiative) => String(initiative.id)).sort() ?? [], [graph]);
  const idsKey = JSON.stringify(ids);
  const read = useMemo(() => access.ready && idsKey !== "[]" ? async () => {
    for (const id of JSON.parse(idsKey) as string[]) await app.manager.readInitiative(access.conn, id);
  } : null, [app, access.conn, access.ready, idsKey]);
  const progressSignal = useSettled(graph?.asOf, 1_000);
  const progressRead = useStoreRead(read, progressSignal);
  const selectDetails = useMemo(() => (state: State) => state.archon[access.conn]?.initiatives ?? NO_DETAILS, [access.conn]);
  const details = useSelector(selectDetails);
  const members = useMemo(() => progressRead.failure ? null : progressMembers(JSON.parse(idsKey) as string[], details, items), [idsKey, details, items, progressRead.failure]);
  if (!access.has("task_graph") && access.ready) {
    return <EmptyState title="No task graph on this Archon">This archond does not serve the task graph. Update it to 1.5 or later to see how items depend on each other.</EmptyState>;
  }
  if (graph === null) {
    return <div className="dag" aria-busy="true" aria-label="Loading the task graph"><Skeleton height="1.5rem" width="40%" /><Skeleton height="28rem" /></div>;
  }
  return <DagWorkspace graph={graph} items={items} layout={layout} progressItems={members} progressFailure={progressRead.failure?.message ?? null} retryProgress={progressRead.reload} />;
}

/** `layout` is for tests, which count layouts; the route renders the worker's layered layout. */
export default function DagScreen({ layout = layeredLayout }: { readonly layout?: LayoutFn }) {
  invariant(typeof layout === "function", "the DAG screen lays out with a function");
  const info = useArchonConnection();
  return info ? <Graph info={info} layout={layout} /> : <NotConnected />;
}
