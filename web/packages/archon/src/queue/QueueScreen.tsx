// `#/archon/queue`: every item of the archond connection, filtered by state, repository, pipeline,
// priority and text, with the waiting reasons and spend. Rows come from core's store, which the change
// stream keeps current; against protocol 23 core refetches them after feed items instead.

import { routeHref, selectItems, useSelector, type ArchonItem } from "@aether-ui/core";
import type { archon, ConnectionInfo } from "@aether-ui/protocol";
import { Badge, Button, EmptyState, StatusPill, VirtualList } from "@aether-ui/ui";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { useActivity, useArchonAccess, useArchonConnection, useRead, useSettled, type ArchonAccess } from "./archon.ts";
import {
  age, compareRows, FILTERS, inScope, invariant, NO_QUERY, PRIORITIES, priorityLabel, queueRow, statePill, usd,
  type QueueQuery, type QueueRow,
} from "./model.ts";
import "./queue.css";
import { QueueStatus } from "./QueueStatus.tsx";

export const ROW_HEIGHT = 68;

/** The current time, moved on once a minute so ages stay true. */
export function useNow(stepMs = 60_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), stepMs);
    return () => clearInterval(timer);
  }, [stepMs]);
  return now;
}

/** Archon screens mount only with an archond connection (ARCHITECTURE.md §a, §b). */
export function NotConnected() {
  return <EmptyState title="Archon not connected">Connect or pair an archond to see its queue.</EmptyState>;
}

export function GithubStates({ row }: { readonly row: Pick<QueueRow, "issue" | "pr"> }) {
  return (
    <>
      {row.issue ? <Badge tone="info">Issue #{row.issue.number}</Badge> : null}
      {row.pr ? <Badge tone="accent">PR #{row.pr.number}{row.pr.state ? ` · ${row.pr.state}` : ""}</Badge> : null}
    </>
  );
}

const QueueRowView = memo(function QueueRowView({ row, repo, now }: { readonly row: QueueRow; readonly repo: string; readonly now: number }) {
  const pill = statePill(row.kind, row.waitsOnYou);
  const meta = [repo, row.pipeline, row.stage && row.kind === "running" ? `stage ${row.stage}` : null, age(row.updatedAt, now)].filter(Boolean).join(" · ");
  const note = row.blockedReason ?? row.waitingText ?? (row.dependencies > 0 ? `Depends on ${row.dependencies} item${row.dependencies > 1 ? "s" : ""}` : null);
  return (
    <div className="queue-row" data-item={row.id}>
      <div className="queue-row-main">
        <a className="queue-title ui-link" href={routeHref("#/archon/items/:item", { item: row.id })}>{row.title}</a>
        <span className="queue-meta">{meta}</span>
        {note ? <span className="queue-note" data-you={row.waitsOnYou ? "true" : undefined}>{note}</span> : null}
      </div>
      <div className="queue-row-side">
        <StatusPill status={pill.status} label={pill.label} />
        {row.priority === "high" || row.priority === "critical" ? <Badge tone={row.priority === "critical" ? "danger" : "warning"}>{priorityLabel(row.priority)}</Badge> : null}
        <GithubStates row={row} />
        {row.spentUsd > 0 ? <span className="queue-spend">{usd(row.spentUsd)}</span> : null}
      </div>
    </div>
  );
});

function Select(props: { readonly label: string; readonly value: string; readonly all: string; readonly options: readonly (readonly [string, string])[]; readonly onChange: (value: string) => void }) {
  return (
    <label className="queue-select">
      <span className="ui-field-label">{props.label}</span>
      <select className="ui-field-input" value={props.value} onChange={(event) => props.onChange(event.target.value)}>
        <option value="">{props.all}</option>
        {props.options.map(([value, text]) => <option key={value} value={value}>{text}</option>)}
      </select>
    </label>
  );
}

function QueueFilters({ query, onChange, counts, repos, pipelines }: {
  readonly query: QueueQuery; readonly onChange: (query: QueueQuery) => void; readonly counts: readonly number[];
  readonly repos: readonly (readonly [string, string])[]; readonly pipelines: readonly string[];
}) {
  invariant(counts.length === FILTERS.length, "every state chip has a count");
  return (
    <div className="queue-filters">
      <div className="queue-chips" role="group" aria-label="Filter by state">
        {FILTERS.map((filter, index) => (
          <button key={filter.id} type="button" className="queue-chip" aria-pressed={query.filter === filter.id} onClick={() => onChange({ ...query, filter: filter.id })}>
            {filter.label} <span className="queue-count">{counts[index]}</span>
          </button>
        ))}
      </div>
      <div className="queue-selects">
        <label className="queue-select queue-search">
          <span className="ui-field-label">Search</span>
          <input className="ui-field-input" type="search" value={query.text} placeholder="Title, brief, id or repository" onChange={(event) => onChange({ ...query, text: event.target.value })} />
        </label>
        <Select label="Repository" all="All repositories" value={query.repo} options={repos} onChange={(repo) => onChange({ ...query, repo })} />
        <Select label="Pipeline" all="All pipelines" value={query.pipeline} options={pipelines.map((name) => [name, name] as const)} onChange={(pipeline) => onChange({ ...query, pipeline })} />
        <Select label="Priority" all="Any priority" value={query.priority} options={PRIORITIES.map((p) => [p, priorityLabel(p)] as const)} onChange={(priority) => onChange({ ...query, priority })} />
      </div>
    </div>
  );
}

function QueueList({ rows, names, now }: { readonly rows: readonly QueueRow[]; readonly names: ReadonlyMap<string, string>; readonly now: number }) {
  const renderRow = useCallback((index: number) => {
    const row = rows[index];
    invariant(row !== undefined, `the list asked for row ${index} of ${rows.length}`);
    return <QueueRowView row={row} repo={row.repo === null ? "no repository" : names.get(row.repo) ?? row.repo} now={now} />;
  }, [rows, names, now]);
  const rowKey = useCallback((index: number) => rows[index]?.id ?? index, [rows]);
  return <div className="queue-list"><VirtualList label="Items" count={rows.length} rowHeight={ROW_HEIGHT} renderRow={renderRow} rowKey={rowKey} overscan={6} /></div>;
}

/** The newer of core's summary and a reread one; the live state, stage and reason stay core's. */
export function withNewerSummary(item: ArchonItem, fresh: archon.ItemSummary | undefined): ArchonItem {
  if (fresh === undefined) return item;
  invariant(typeof fresh.updated_at === "string", `the reread summary of ${fresh.id} has a time`);
  const held = Date.parse(String(item.summary.updated_at));
  return Number.isFinite(held) && held > Date.parse(fresh.updated_at) ? item : { ...item, summary: fresh as unknown as ArchonItem["summary"] };
}

/**
 * The rows. Change records move an item's state, stage and reason but not its summary (spend, prompt,
 * PR), so at protocol 24 the summaries are reread once activity settles; at 23 core rereads them itself.
 */
function useRows(access: ArchonAccess, settled: number): { items: Readonly<Record<string, ArchonItem>>; rows: QueueRow[] } {
  const items = useSelector(useMemo(() => selectItems(access.conn), [access.conn]));
  const fresh = useRead<archon.ItemSummary[]>(access, access.live && settled > 0 ? { cmd: "list_item_summaries" } : null, ["item_summaries"], settled);
  const rows = useMemo(() => {
    const byId = new Map((fresh.data ?? []).map((summary) => [summary.id, summary]));
    return Object.entries(items).map(([id, item]) => queueRow(id, withNewerSummary(item, byId.get(id)))).sort(compareRows);
  }, [items, fresh.data]);
  return { items, rows };
}

function Queue({ info }: { readonly info: ConnectionInfo }) {
  const access = useArchonAccess(info);
  const settled = useSettled(useActivity(access.conn), 1_000);
  const { items, rows } = useRows(access, settled);
  const repos = useRead<archon.Repo[]>(access, { cmd: "repo_list" }, ["repos"]);
  const names = useMemo(() => new Map((repos.data ?? []).map((repo) => [repo.id, repo.name])), [repos.data]);
  const [query, setQuery] = useState<QueueQuery>(NO_QUERY);
  const now = useNow();
  const scoped = useMemo(() => rows.filter((row) => inScope(row, query, names)), [rows, query, names]);
  const counts = useMemo(() => FILTERS.map((filter) => scoped.filter(filter.test).length), [scoped]);
  const active = FILTERS.find((filter) => filter.id === query.filter) ?? FILTERS[0];
  invariant(active !== undefined, "the queue has a state filter");
  const shown = useMemo(() => scoped.filter(active.test), [scoped, active]);
  const pipelines = useMemo(() => [...new Set(rows.map((row) => row.pipeline))].sort(), [rows]);
  return (
    <section className="queue" aria-label="Queue">
      <QueueStatus access={access} items={items} names={names} settled={settled} />
      <QueueFilters query={query} onChange={setQuery} counts={counts} repos={[...names.entries()]} pipelines={pipelines} />
      <p className="queue-total" aria-live="polite">{shown.length} of {rows.length} items{access.live ? "" : " · refreshed after each feed event (protocol 23)"}</p>
      {shown.length > 0 ? <QueueList rows={shown} names={names} now={now} />
        : rows.length === 0 ? <EmptyState title="Nothing queued yet">Queue work with New item.</EmptyState>
          : <EmptyState title="No item matches" action={<Button onClick={() => setQuery(NO_QUERY)}>Clear filters</Button>}>Every filter must match.</EmptyState>}
    </section>
  );
}

export function QueueScreen() {
  const info = useArchonConnection();
  return info ? <Queue info={info} /> : <NotConnected />;
}

export default function QueueRoute() {
  return <QueueScreen />;
}
