// `#/fleet`: every agent of every Aether connection in one virtualized list, filtered by run,
// role, state and model. core pages the agents in (`list_agents_page`); the list draws only what is on screen.

import { routeHref, selectConnections, useApp, useDraft, useSelector } from "@aether-ui/core";
import type { ConnectionInfo } from "@aether-ui/protocol";
import { Badge, Button, EmptyState, Picker, StatusPill, VirtualList } from "@aether-ui/ui";
import { memo, useCallback, useMemo, useState } from "react";
import "./fleet.css";
import { StartAgentButton } from "../activity/StartAgent.tsx";
import { useArchived } from "./useArchived.ts";
import { FleetFilters } from "./FleetFilters.tsx";
import { invariant } from "./invariant.ts";
import { roleLabel } from "./roles.ts";
import {
  agentStatus, filterRows, fleetFacets, fleetRows, modelName, NO_FILTER, runKey, runNames, sameSources, selectFleetSources,
  type FleetRow,
} from "./rows.ts";

export const ROW_HEIGHT = 44;

function sameArchiveConnections(a: readonly ConnectionInfo[], b: readonly ConnectionInfo[]): boolean {
  return a.length === b.length && a.every((info, i) => info.id === b[i]?.id
    && info.features.includes("agents_paging") === b[i]?.features.includes("agents_paging"));
}

function sameReadyConnections(a: ReturnType<typeof selectConnections>, b: ReturnType<typeof selectConnections>): boolean {
  return sameArchiveConnections(
    Object.values(a).filter(info => info.daemon === "aether" && info.state === "ready"),
    Object.values(b).filter(info => info.daemon === "aether" && info.state === "ready"),
  );
}

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
  const connections = useSelector(selectConnections, sameReadyConnections);
  const app = useApp();
  const rows = useMemo(() => fleetRows(sources), [sources]);
  const archiveDraft = useDraft("fleet/archive", { value: "active" });
  const archive = archiveDraft.value.value;
  const [archiveToken, setArchiveToken] = useState(0);
  const archived = useArchived(app, archive === "archived", archiveToken, connections, sources);
  const ready = Object.values(connections).some(info => info.daemon === "aether" && info.state === "ready");
  const visible = useMemo(() => rows.filter(row => archive === "archived" ? row.agent.archived : !row.agent.archived), [rows, archive]);
  const names = useMemo(() => runNames(visible), [visible]);
  const facets = useMemo(() => fleetFacets(visible, names), [visible, names]);
  const draft = useDraft("fleet/filters", { ...NO_FILTER });
  const filter = draft.value;
  const setFilter = draft.update;
  const shown = useMemo(() => filterRows(visible, filter), [visible, filter]);
  const clear = draft.clear;

  if (sources.length === 0) {
    return <EmptyState title="No Aether daemon connected">The fleet lists the agents of every connected Aether daemon.</EmptyState>;
  }
  return (
    <section className="fleet" aria-label="Fleet">
      <div className="fleet-bar">
        <Picker label="Archive" size="sm" value={archive} onChange={value => { archiveDraft.update({ value }); setArchiveToken(n => n + 1); clear(); }}
          options={[{ id: "active", label: "Active fleet" }, { id: "archived", label: "Archived" }]} />
        <FleetFilters facets={facets} filter={filter} onChange={setFilter} />
        <p className="fleet-count" aria-live="polite">{shown.length.toLocaleString("en")} of {visible.length.toLocaleString("en")} agents</p>
        <a className="ui-link" href="#/fleet/topology">Delegation topology</a>
      </div>
      {archived.error ? <div role="alert">Could not load archived agents: {archived.error} <Button onClick={archived.retry}>Retry</Button></div> : null}
      {archived.loading ? <p role="status">{ready ? "Loading archived agents…" : "Waiting for an Aether connection…"}</p> : null}
      {shown.length > 0 ? <FleetList rows={shown} names={names} />
        : archived.loading || archived.error || archived.hasMore ? null : visible.length === 0 ? archive === "archived" ? <EmptyState title="No archived agents">Agents you archive appear here.</EmptyState> : <EmptyState title="No agents yet" action={<StartAgentButton conn={sources[0]?.conn ?? ""} label="Start your first agent" />}>Choose an objective and workspace to begin.</EmptyState>
          : <EmptyState title="No agent matches" action={<Button onClick={clear}>Clear filters</Button>}>Every filter must match.</EmptyState>}
      {archived.hasMore ? <Button disabled={archived.loading} onClick={archived.more}>Load more</Button> : null}
    </section>
  );
}

/** The route's screen; `#/fleet` has no params (RouteProps). */
export default function FleetRoute() {
  return <FleetScreen />;
}
