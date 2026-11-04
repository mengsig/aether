// `#/fleet`: every agent of every Aether connection in one virtualized list, filtered by run,
// role, state and model. core pages the agents in (`list_agents_page`); the list draws only what is on screen.

import { routeHref, useSelector } from "@aether-ui/core";
import { Badge, Button, EmptyState, StatusPill, VirtualList } from "@aether-ui/ui";
import { memo, useCallback, useMemo, useState } from "react";
import "./fleet.css";
import { FleetFilters } from "./FleetFilters.tsx";
import { invariant } from "./invariant.ts";
import { roleLabel } from "./roles.ts";
import {
  agentStatus, filterRows, fleetFacets, fleetRows, modelName, NO_FILTER, runKey, runNames, sameSources, selectFleetSources,
  type FleetFilter, type FleetRow,
} from "./rows.ts";

export const ROW_HEIGHT = 44;

export function RoleTag({ role }: { readonly role: string }) {
  const label = roleLabel(role);
  if (label.kind === "agent") return <span className="fleet-role" title={role}>{label.text}</span>;
  return <span className="fleet-role" title={role}><Badge tone="accent">{label.text}</Badge></span>;
}

// Memoized: a scroll step renders only the rows that came into view, not the whole screenful.
const FleetRowView = memo(function FleetRowView({ row, runName }: { readonly row: FleetRow; readonly runName: string }) {
  const { agent } = row;
  const status = agentStatus(agent);
  return (
    <div className="fleet-row" data-agent={agent.id}>
      <a className="fleet-name ui-link" href={routeHref("#/agents/:conn/:agent", { conn: row.conn, agent: agent.id })}>{agent.name}</a>
      <RoleTag role={agent.role} />
      <span className="fleet-state">
        <StatusPill status={status.status} label={status.label} />
        {agent.paused ? <Badge tone="warning">Paused</Badge> : null}
      </span>
      <code className="fleet-model">{modelName(agent)}</code>
      <span className="fleet-run" title={agent.run}>{runName}</span>
    </div>
  );
});

function FleetList({ rows, names }: { readonly rows: readonly FleetRow[]; readonly names: ReadonlyMap<string, string> }) {
  const renderRow = useCallback((index: number) => {
    const row = rows[index];
    invariant(row !== undefined, `the list asked for row ${index} of ${rows.length}`);
    return <FleetRowView row={row} runName={names.get(runKey(row.conn, row.agent.run)) ?? row.agent.run} />;
  }, [rows, names]);
  const rowKey = useCallback((index: number) => rows[index]?.key ?? index, [rows]);
  return (
    <div className="fleet-table">
      <div className="fleet-row fleet-head" aria-hidden="true">
        <span>Agent</span><span>Role</span><span>State</span><span>Model</span><span>Run</span>
      </div>
      <div className="fleet-list">
        <VirtualList label="Agents" count={rows.length} rowHeight={ROW_HEIGHT} renderRow={renderRow} rowKey={rowKey} overscan={6} />
      </div>
    </div>
  );
}

export function FleetScreen() {
  const sources = useSelector(selectFleetSources, sameSources);
  const rows = useMemo(() => fleetRows(sources), [sources]);
  const names = useMemo(() => runNames(rows), [rows]);
  const facets = useMemo(() => fleetFacets(rows, names), [rows, names]);
  const [filter, setFilter] = useState<FleetFilter>(NO_FILTER);
  const shown = useMemo(() => filterRows(rows, filter), [rows, filter]);
  const clear = useCallback(() => setFilter(NO_FILTER), []);

  if (sources.length === 0) {
    return <EmptyState title="No Aether daemon connected">The fleet lists the agents of every connected Aether daemon.</EmptyState>;
  }
  return (
    <section className="fleet" aria-label="Fleet">
      <div className="fleet-bar">
        <FleetFilters facets={facets} filter={filter} onChange={setFilter} />
        <p className="fleet-count" aria-live="polite">{shown.length.toLocaleString("en")} of {rows.length.toLocaleString("en")} agents</p>
        <a className="ui-link" href="#/fleet/topology">Delegation topology</a>
      </div>
      {shown.length > 0 ? <FleetList rows={shown} names={names} />
        : rows.length === 0 ? <EmptyState title="No agents yet">Agents appear here as soon as a daemon creates them.</EmptyState>
          : <EmptyState title="No agent matches" action={<Button onClick={clear}>Clear filters</Button>}>Every filter must match.</EmptyState>}
    </section>
  );
}

/** The route's screen; `#/fleet` has no params (RouteProps). */
export default function FleetRoute() {
  return <FleetScreen />;
}
