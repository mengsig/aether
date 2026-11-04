// `#/fleet/topology`: who delegated to whom, grouped by run. A run collapses into one node; the
// layout is asked for only when the structure changes, never for a status change (LayoutGate).

import { navigate, routeHref, useSelector } from "@aether-ui/core";
import { GraphCanvas, layeredLayout } from "@aether-ui/graph";
import type { Layout } from "@aether-ui/protocol";
import { Button, EmptyState, ErrorState } from "@aether-ui/ui";
import { useCallback, useEffect, useMemo, useState } from "react";
import { sameSources, selectFleetSources } from "../fleet/rows.ts";
import { buildTopology, collapsedRuns, toggledForAll, topologyModel } from "./graph.ts";
import { LayoutGate, type LayoutFn } from "./relayout.ts";
import "./topology.css";

const NO_TOGGLES: ReadonlySet<string> = new Set();

const errorText = (error: unknown): string => (error instanceof Error && error.message !== "" ? error.message : String(error) || "the layout failed");

export interface TopologyScreenProps {
  /** The layout behind the graph; tests pass a counting one. */
  readonly layout?: LayoutFn;
}

export function TopologyScreen({ layout = layeredLayout }: TopologyScreenProps) {
  const sources = useSelector(selectFleetSources, sameSources);
  const topology = useMemo(() => buildTopology(sources), [sources]);
  const [toggled, setToggled] = useState<ReadonlySet<string>>(NO_TOGGLES);
  const model = useMemo(() => topologyModel(topology, collapsedRuns(topology, toggled)), [topology, toggled]);
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

  const toggle = useCallback((group: string) => setToggled((old) => {
    const next = new Set(old);
    if (!next.delete(group)) next.add(group);
    return next;
  }), []);
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
        <Button onClick={() => setToggled(toggledForAll(topology, false))}>Expand all runs</Button>
        <Button onClick={() => setToggled(toggledForAll(topology, true))}>Collapse all runs</Button>
        <a className="ui-link" href="#/fleet">Fleet list</a>
      </div>
      <div className="topology-canvas">
        <GraphCanvas model={model} layout={placed} label="Delegation topology" onActivate={activate} onToggleGroup={toggle} />
      </div>
    </section>
  );
}

/** The route's screen; `#/fleet/topology` has no params. */
export default function TopologyRoute() {
  return <TopologyScreen />;
}
