// `#/fleet/topology`: who delegated to whom, grouped by run. A run collapses into one node; the
// layout is asked for only when the structure changes, never for a status change (LayoutGate).

import { navigate, routeHref, useSelector } from "@aether-ui/core";
import { GraphCanvas, layeredLayout } from "@aether-ui/graph";
import type { Layout } from "@aether-ui/protocol";
import { Button, EmptyState, ErrorState } from "@aether-ui/ui";
import { useCallback, useEffect, useMemo, useState } from "react";
import { sameSources, selectFleetSources } from "../fleet/rows.ts";
import { buildTopology, collapsedRuns, toggleRun, topologyModel, type RunOverrides } from "./graph.ts";
import { LayoutGate, type LayoutFn } from "./relayout.ts";
import { RunList } from "./RunList.tsx";
import { selectRun, summarizeRuns } from "./runs.ts";
import "./topology.css";

const NO_OVERRIDES: RunOverrides = new Map();

const errorText = (error: unknown): string => (error instanceof Error && error.message !== "" ? error.message : String(error) || "the layout failed");

export interface TopologyScreenProps {
  /** The layout behind the graph; tests pass a counting one. */
  readonly layout?: LayoutFn;
}

export function TopologyScreen({ layout = layeredLayout }: TopologyScreenProps) {
  const sources = useSelector(selectFleetSources, sameSources);
  const topology = useMemo(() => buildTopology(sources), [sources]);
  const runs = useMemo(() => summarizeRuns(topology, sources), [topology, sources]);
  const [chosen, setChosen] = useState(() => runs[0]?.id);
  const selected = runs.some((run) => run.id === chosen) ? chosen : runs[0]?.id;
  const visible = useMemo(() => selected === undefined ? topology : selectRun(topology, selected), [topology, selected]);
  const [overrides, setOverrides] = useState<RunOverrides>(NO_OVERRIDES);
  const model = useMemo(() => topologyModel(visible, collapsedRuns(visible, overrides)), [visible, overrides]);
  const gate = useMemo(() => new LayoutGate(layout), [layout]);
  const [placed, setPlaced] = useState<Layout>();
  const [failure, setFailure] = useState<string>();
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let live = true;
    gate.layout(model).then(
      (next) => { if (live) { setPlaced(next); setFailure(undefined); } },
      (error: unknown) => { if (live) setFailure(errorText(error)); },
    );
    return () => { live = false; };
  }, [gate, model, attempt]);

  const toggle = useCallback((group: string) => setOverrides((old) => toggleRun(topology, old, group)), [topology]);
  const activate = useCallback((id: string) => {
    const ref = topology.agentOf.get(id);
    if (ref !== undefined) navigate(routeHref("#/agents/:conn/:agent", { conn: ref.conn, agent: ref.agent }));
  }, [topology]);

  if (sources.length === 0) {
    return <EmptyState title="No Aether daemon connected">The topology shows how the agents of every connected Aether daemon delegate.</EmptyState>;
  }
  if (topology.nodes.length === 0) return <EmptyState title="No agents yet">Agents and their delegations appear here as daemons create them.</EmptyState>;
  if (failure !== undefined) return <ErrorState title="The topology could not be laid out" message={failure} onRetry={() => setAttempt((n) => n + 1)} />;
  return (
    <section className="topology" aria-label="Delegation topology">
      <div className="topology-bar">
        <p className="topology-count">{topology.nodes.length.toLocaleString("en")} agents in {topology.groups.length.toLocaleString("en")} runs</p>
        <Button onClick={() => { if (selected !== undefined) setOverrides((old) => new Map(old).set(selected, false)); }}>Expand run</Button>
        <Button onClick={() => { if (selected !== undefined) setOverrides((old) => new Map(old).set(selected, true)); }}>Collapse run</Button>
        <a className="ui-link" href="#/fleet">Fleet list</a>
      </div>
      <div className="topology-content">
        <RunList runs={runs} selected={selected} onSelect={setChosen} />
        <div className="topology-canvas">
          <GraphCanvas key={selected} model={model} layout={placed} label="Delegation topology" onActivate={activate} onToggleGroup={toggle}
            renderControls={({ zoomIn, zoomOut, fit }) => <div className="topology-zoom" role="group" aria-label="Graph zoom">
              <Button onClick={zoomIn}>Zoom in</Button><Button onClick={zoomOut}>Zoom out</Button><Button onClick={fit}>Fit</Button>
            </div>} />
        </div>
      </div>
    </section>
  );
}

/** The route's screen; `#/fleet/topology` has no params. */
export default function TopologyRoute() {
  return <TopologyScreen />;
}
