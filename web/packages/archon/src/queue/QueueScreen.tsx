// `#/archon/queue`: every item of the archond connection, filtered by state, repository, pipeline,
// priority and text, with the waiting reasons and spend. Rows come from core's store, which the change
// stream keeps current; against protocol 23 core refetches them after feed items instead.

import { connectionDraftKey, routeHref, selectItems, useSelector, useDraft, type ArchonItem } from "@aether-ui/core";
import type { archon, ConnectionInfo } from "@aether-ui/protocol";
import { Badge, Button, EmptyState, ErrorState, Field, Picker, StatusPill, VirtualList } from "@aether-ui/ui";
import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { useActivity, useArchonAccess, useArchonConnection, useRead, useSettled } from "./archon.ts";
import {
  age, compareRows, FILTERS, inScope, invariant, NO_QUERY, PRIORITIES, priorityLabel, queueRow, statePill, usd,
  type QueueQuery, type QueueRow,
} from "./model.ts";
import "./queue.css";
import { EMPTY_ITEM, itemDraftKey, NewItemDialog, SayDialog } from "./NewItemDialog.tsx";
import { RegisterRepository } from "./RegisterRepository.tsx";
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
  return <Picker label={props.label} size="sm" value={props.value} onChange={props.onChange}
    data-testid={`queue-filter-${props.label === "Repository" ? "repo" : props.label.toLowerCase()}`}
    options={[{ id: "", label: props.all }, ...props.options.map(([id, label]) => ({ id, label }))]} />;
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
        <Field label="Search" className="queue-search" type="search" value={query.text} placeholder="Title, brief, id or repository" onChange={(event) => onChange({ ...query, text: event.target.value })} />
        <Picker label="State" size="sm" value={query.filter} data-testid="queue-filter-state" onChange={(filter) => onChange({ ...query, filter: filter as QueueQuery["filter"] })}
          options={FILTERS.map((filter, index) => ({ id: filter.id, label: filter.label, badge: String(counts[index]) }))} />
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

/** The rows, derived from core's store alone: the change stream and core's summary rereads keep it current. */
function useRows(conn: string): { items: Readonly<Record<string, ArchonItem>>; rows: QueueRow[] } {
  const items = useSelector(useMemo(() => selectItems(conn), [conn]));
  const rows = useMemo(() => Object.entries(items).map(([id, item]) => queueRow(id, item)).sort(compareRows), [items]);
  return { items, rows };
}

function Queue({ info }: { readonly info: ConnectionInfo }) {
  const access = useArchonAccess(info);
  const activity = useActivity(access.conn);
  const settled = useSettled(activity, 1_000);
  const { items, rows } = useRows(access.conn);
  const repos = useRead<archon.Repo[]>(access, { cmd: "repo_list" }, ["repos"], settled);
  const catalog = useRead<archon.CatalogListing>(access, { cmd: "list_pipelines", repo: null }, ["catalog"], settled);
  const [dialog, setDialog] = useState<"item" | "say" | "repo" | null>(null);
  const [registered, setRegistered] = useState<archon.Repo | null>(null);
  const itemDraft = useDraft(itemDraftKey(access.conn), EMPTY_ITEM);
  const available = useMemo(() => registered && !repos.data?.some((repo) => repo.id === registered.id) ? [...(repos.data ?? []), registered] : repos.data ?? [], [repos.data, registered]);
  const names = useMemo(() => new Map(available.map((repo) => [repo.id, repo.name])), [available]);
  const { value: query, update: setQuery } = useDraft(connectionDraftKey("archon", access.conn, "queue-filters"), { ...NO_QUERY });
  const now = useNow();
  const scoped = useMemo(() => rows.filter((row) => inScope(row, query, names)), [rows, query, names]);
  const counts = useMemo(() => FILTERS.map((filter) => scoped.filter(filter.test).length), [scoped]);
  const active = FILTERS.find((filter) => filter.id === query.filter) ?? FILTERS[0];
  invariant(active !== undefined, "the queue has a state filter");
  const shown = useMemo(() => scoped.filter(active.test), [scoped, active]);
  const pipelines = useMemo(() => [...new Set([...(catalog.data?.pipelines ?? []).map((pipeline) => pipeline.name), ...rows.map((row) => row.pipeline)])].sort(), [rows, catalog.data]);
  return (
    <section className="queue" aria-label="Queue">
      <QueueStatus access={access} items={items} names={names} settled={settled} onDialog={setDialog} />
      <QueueFilters query={query} onChange={setQuery} counts={counts} repos={[...names.entries()]} pipelines={pipelines} />
      <p className="queue-total" aria-live="polite">{shown.length} of {rows.length} items{access.live ? "" : " · refreshed after each feed event (protocol 23)"}</p>
      {shown.length > 0 ? <QueueList rows={shown} names={names} now={now} />
        : repos.data?.length === 0 ? <EmptyState title="Register a repository" action={access.refusal === null ? <Button variant="primary" data-testid="repo-register" onClick={() => setDialog("repo")}>Register a repository</Button> : undefined}>Choose a checkout and its pipeline to queue your first item. {access.refusal ? "Ask an operator to register it." : ""}</EmptyState>
          : rows.length === 0 ? <EmptyState title="Nothing queued yet">{access.refusal ? "Ask an operator to queue work." : "Queue work with New item."}</EmptyState>
          : <EmptyState title="No item matches" action={<Button onClick={() => setQuery(NO_QUERY)}>Clear filters</Button>}>Every filter must match.</EmptyState>}
      {repos.error ? <ErrorState title="Could not read repositories" message={repos.error} onRetry={repos.reload} /> : null}
      <NewItemDialog access={access} open={dialog === "item"} repos={available} onClose={() => setDialog(null)} />
      <SayDialog access={access} open={dialog === "say"} onClose={() => setDialog(null)} />
      <RegisterRepository access={access} open={dialog === "repo"} onClose={() => setDialog(null)} onRegistered={(repo) => {
        setRegistered(repo); repos.reload(); itemDraft.update({ repo: repo.id, pipeline: repo.default_pipeline ?? "" }); setDialog("item");
      }} />
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
