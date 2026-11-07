// `#/archon/initiatives/:id`: one initiative's contract, its members with their live states and why
// they are held, and the order they run in. The detail, items and graph all come from core's store.

import { routeHref, selectInitiative, selectItems, selectTaskGraph, useApp, useSelector } from "@aether-ui/core";
import { layeredLayout } from "@aether-ui/graph";
import type { archon, ConnectionInfo, RouteProps } from "@aether-ui/protocol";
import { Badge, Button, EmptyState, ErrorState, Skeleton, StatusPill } from "@aether-ui/ui";
import { useCallback, useMemo, useState } from "react";
import { DagCanvas } from "../dag/DagCanvas.tsx";
import type { LayoutFn } from "../dag/layout.ts";
import { criticalPath, dagData, reasonsFor, type DagData } from "../dag/model.ts";
import "../dag/dag.css";
import { useArchonAccess, useArchonConnection, useSettled, type ArchonAccess } from "../queue/archon.ts";
import { dateTime, invariant, usd } from "../queue/model.ts";
import { NotConnected } from "../queue/QueueScreen.tsx";
import { initiativePill, initiativeView, type InitiativeView, type MemberRow } from "./model.ts";
import { useStoreRead } from "./read.ts";
import "./initiatives.css";

/** A change settles this long before the initiative is read again, so a burst rereads once. */
export const SETTLE_MS = 500;

/**
 * Moves when the initiative or one of its members does in core's store: core replaces the initiative's
 * graph entry on each of its change frames, and members move in core's items. The detail is reread
 * when it settles.
 */
function useInitiativeSignal(conn: string, id: string): object {
  const items = useSelector(useMemo(() => selectItems(conn), [conn]));
  const graph = useSelector(useMemo(() => selectTaskGraph(conn), [conn]));
  const entry = graph?.initiatives.find((initiative) => initiative.id === id);
  const members = useMemo(() => Object.entries(items).filter(([, item]) => item.summary.initiative === id)
    .map(([key, item]) => `${key}:${item.state}`).sort().join("|"), [items, id]);
  // A new token for a new entry object or member set, even when its fields read the same.
  const token = useMemo(() => ({ entry, members }), [entry, members]);
  return useSettled(token, SETTLE_MS);
}

function Members({ view, graph, selected, onSelect }: { readonly view: InitiativeView; readonly graph: DagData; readonly selected: string | undefined; readonly onSelect: (id: string) => void }) {
  if (view.members.length === 0) return <p className="dag-muted">This initiative has no items yet.</p>;
  return (
    <ul className="initiative-members">
      {view.members.map((row) => (
        <MemberItem key={row.id} row={row} reasons={graph.byId.has(row.id) ? reasonsFor(graph, row.id) : []} selected={row.id === selected} onSelect={onSelect} />
      ))}
    </ul>
  );
}

function MemberItem({ row, reasons, selected, onSelect }: { readonly row: MemberRow; readonly reasons: readonly string[]; readonly selected: boolean; readonly onSelect: (id: string) => void }) {
  return (
    <li className="initiative-member" data-item={row.id} aria-current={selected ? "true" : undefined}>
      <button type="button" className="dag-row" onClick={() => onSelect(row.id)}>
        <span className="dag-row-title">{row.title}</span>
        <StatusPill status={row.status} label={row.label} />
        {reasons.length > 0 ? <span className="dag-reason">{reasons.join(" · ")}</span> : null}
      </button>
      <a className="ui-link initiative-open" href={routeHref("#/archon/items/:item", { item: row.id })} aria-label={`Open ${row.title}`}>Open</a>
    </li>
  );
}

function Header({ view }: { readonly view: InitiativeView }) {
  const { initiative } = view;
  const pill = initiativePill(initiative.state);
  const total = view.members.length;
  return (
    <header className="initiative-header">
      <a className="ui-link ui-back initiative-back" href="#/archon/graph">← Task graph</a>
      <p className="ui-eyebrow initiative-eyebrow">Initiative</p>
      <h1 className="ui-page-title initiative-title">{initiative.title}</h1>
      <div className="dag-badges">
        <StatusPill status={pill.status} label={pill.label} />
        {initiative.atomic === true ? <Badge tone="accent">Atomic</Badge> : null}
        <span className="dag-muted">Updated {dateTime(initiative.updated_at)}</span>
      </div>
      <dl className="initiative-facts">
        <div><dt>Done</dt><dd>{view.done} of {total}</dd></div>
        <div><dt>Open</dt><dd>{view.open}</dd></div>
        <div><dt>Failed</dt><dd>{view.failed}</dd></div>
        <div><dt>Spent</dt><dd>{usd(view.spentUsd)}</dd></div>
      </dl>
      <meter className="dag-meter" min={0} max={Math.max(1, total)} value={view.done} aria-label={`${view.done} of ${total} done`} />
    </header>
  );
}

function Order({ data, layout, selected, onSelect }: { readonly data: DagData; readonly layout: LayoutFn; readonly selected: string | undefined; readonly onSelect: (id: string) => void }) {
  const path = useMemo(() => criticalPath(data), [data]);
  if (data.nodes.length === 0) return <p className="dag-muted">None of its items is in the task graph.</p>;
  return (
    <>
      <p className="dag-muted">{path.nodes.length > 1 ? `The critical path runs through ${path.nodes.length} items.` : "No open item waits on another."}</p>
      <DagCanvas data={data} path={path} layout={layout} label="Initiative graph" selected={selected} onSelect={onSelect} />
    </>
  );
}

export interface InitiativePageProps {
  readonly detail: archon.InitiativeDetail;
  readonly access: ArchonAccess;
  readonly layout: LayoutFn;
}

export function InitiativePage({ detail, access, layout }: InitiativePageProps) {
  const id = detail.initiative.id;
  const items = useSelector(useMemo(() => selectItems(access.conn), [access.conn]));
  const graph = useSelector(useMemo(() => selectTaskGraph(access.conn), [access.conn]));
  const view = useMemo(() => initiativeView(detail, items), [detail, items]);
  const data = useMemo(() => graph === null ? null : dagData(graph, (node) => node.initiative === id, items), [graph, id, items]);
  const [picked, setPicked] = useState<string>();
  const selected = picked !== undefined && view.members.some((row) => row.id === picked) ? picked : undefined;
  const onSelect = useCallback((item: string) => setPicked(item), []);
  return (
    <article className="ui-page initiative" data-initiative={id}>
      <Header view={view} />
      <div className="initiative-body">
        <section className="ui-card initiative-card" aria-label="Contract">
          <h2 className="dag-heading">Contract</h2>
          {detail.initiative.contract.trim() === "" ? <p className="dag-muted">No contract was written.</p>
            : <p className="initiative-contract">{detail.initiative.contract}</p>}
        </section>
        <section className="ui-card initiative-card" aria-label="Members">
          <h2 className="dag-heading">Items <span className="dag-count">{view.members.length}</span></h2>
          <Members view={view} graph={data ?? EMPTY_DATA} selected={selected} onSelect={onSelect} />
        </section>
        <section className="initiative-card initiative-order" aria-label="Order">
          <h2 className="dag-heading">Order</h2>
          {data === null ? <Skeleton height="16rem" /> : <Order data={data} layout={layout} selected={selected} onSelect={onSelect} />}
        </section>
      </div>
    </article>
  );
}

const EMPTY_DATA: DagData = dagData({ nodes: {}, edges: [], initiatives: [], asOf: 0, truncated: false });

function Initiative({ info, id, layout }: { readonly info: ConnectionInfo; readonly id: string; readonly layout: LayoutFn }) {
  const app = useApp();
  const access = useArchonAccess(info);
  const signal = useInitiativeSignal(access.conn, id);
  const read = useMemo(() => access.ready ? () => app.manager.readInitiative(access.conn, id) : null, [app, access.conn, access.ready, id]);
  const { failure, reload } = useStoreRead(read, signal);
  const stored = useSelector(useMemo(() => selectInitiative(access.conn, id), [access.conn, id]));
  if (failure?.code === "not_found") {
    return <EmptyState title="No such initiative" action={<a className="ui-link" href="#/archon/graph">Back to the task graph</a>}>Archon has no initiative {id}. It may have been removed.</EmptyState>;
  }
  if (stored === undefined) {
    if (failure !== null) return <ErrorState title="The initiative could not be read" message={failure.message} onRetry={reload} />;
    return <div className="ui-page initiative" aria-busy="true" aria-label="Loading the initiative"><Skeleton height="2rem" width="50%" /><Skeleton height="6rem" /><Skeleton height="16rem" /></div>;
  }
  return (
    <>
      {failure === null ? null : <div className="dag-warning" role="status">Showing the last read: {failure.message} <Button variant="quiet" size="sm" onClick={reload}>Try again</Button></div>}
      <InitiativePage detail={stored.value as unknown as archon.InitiativeDetail} access={access} layout={layout} />
    </>
  );
}

/** `layout` is for tests, which count layouts; the route lays out in the worker. */
export default function InitiativeScreen({ params, layout = layeredLayout }: RouteProps & { readonly layout?: LayoutFn }) {
  const id = params.id ?? "";
  invariant(typeof layout === "function", "the initiative page lays out with a function");
  const info = useArchonConnection();
  if (id === "") return <EmptyState title="No initiative named">Open an initiative from the task graph.</EmptyState>;
  return info ? <Initiative info={info} id={id} layout={layout} /> : <NotConnected />;
}
