// `#/fleet/topology`: who delegated to whom, grouped by run. A run collapses into one node; the
// layout is asked for only when the structure changes, never for a status change (LayoutGate).

import { useSelector } from "@aether-ui/core";
import { GraphCanvas, layeredLayout } from "@aether-ui/graph";
import type { Layout } from "@aether-ui/protocol";
import { Button, ErrorState } from "@aether-ui/ui";
import { useCallback, useEffect, useMemo, useState } from "react";
import { fleetRows, sameSources, selectFleetSources } from "../fleet/rows.ts";
import { buildTopology, collapsedRuns, overridesForAll, toggleRun, topologyModel, type RunOverrides } from "./graph.ts";
import { LayoutGate, type LayoutFn } from "./relayout.ts";
import { AgentDetails, agentNodeStyle, AGENT_LEGEND, TopologyEmpty } from "./presentation.tsx";
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
  const rows = useMemo(() => new Map(fleetRows(sources).map((row) => [row.key, row])), [sources]);
  const [overrides, setOverrides] = useState<RunOverrides>(NO_OVERRIDES);
  const model = useMemo(() => topologyModel(topology, collapsedRuns(topology, overrides)), [topology, overrides]);
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
  if (sources.length === 0) return <TopologyEmpty disconnected />;
  if (topology.nodes.length === 0) return <TopologyEmpty />;
  if (failure !== undefined) return <ErrorState title="The topology could not be laid out" message={failure} onRetry={() => setAttempt((n) => n + 1)} />;
  return (
    <section className="topology" aria-label="Delegation topology">
      <div className="topology-bar">
        <p className="topology-count">{topology.nodes.length.toLocaleString("en")} agents in {topology.groups.length.toLocaleString("en")} runs</p>
        <Button onClick={() => setOverrides(overridesForAll(topology, false))}>Expand all runs</Button>
        <Button onClick={() => setOverrides(overridesForAll(topology, true))}>Collapse all runs</Button>
        <a className="ui-link" href="#/fleet">Fleet list</a>
      </div>
      <div className="topology-canvas">
        <GraphCanvas model={model} layout={placed} label="Delegation topology" onToggleGroup={toggle}
          nodeStyle={(node) => agentNodeStyle(rows.get(node.id)?.agent)} legend={AGENT_LEGEND} emptyState={<TopologyEmpty />}
          details={(node) => { const row = rows.get(node.id); return row === undefined ? <strong>{node.label}</strong> : <AgentDetails row={row} />; }} />
      </div>
    </section>
  );
}

/** The route's screen; `#/fleet/topology` has no params. */
export default function TopologyRoute() {
  return <TopologyScreen />;
}
