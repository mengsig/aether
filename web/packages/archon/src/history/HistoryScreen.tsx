// `#/archon/history`: find items by text and when they last moved (`list_items` filters), then read one
// item's transitions (`item_history`). Searches and histories are stored by core; this keeps the query.

import { searchKey, selectItem, selectItems, selectSearch, useApp, useSelector, type ItemSearch } from "@aether-ui/core";
import type { ConnectionInfo } from "@aether-ui/protocol";
import { Button, EmptyState, ErrorState, Field, Skeleton, StatusPill } from "@aether-ui/ui";
import { useCallback, useMemo, useState, type FormEvent } from "react";
import "../dag/dag.css";
import { useStoreRead } from "../initiatives/read.ts";
import { useActivity, useArchonAccess, useArchonConnection, useSettled, type ArchonAccess } from "../queue/archon.ts";
import { dateTime, invariant, isStateKind, statePill } from "../queue/model.ts";
import { NotConnected } from "../queue/QueueScreen.tsx";
import { defaultForm, queryOf, searchOf, SEARCH_LIMIT, type SearchForm } from "./model.ts";
import { Timeline } from "./Timeline.tsx";
import "./history.css";

/** Results are read again this long after archond reports a change, so a burst rereads once. */
export const SETTLE_MS = 1_000;

function SearchBar({ initial, onSearch }: { readonly initial: SearchForm; readonly onSearch: (query: ItemSearch) => void }) {
  const [form, setForm] = useState(initial);
  const [error, setError] = useState<string>();
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const result = searchOf(form);
    if ("error" in result) return setError(result.error);
    setError(undefined);
    onSearch(result.query);
  };
  const field = (name: keyof SearchForm) => ({ value: form[name], onChange: (event: { target: { value: string } }) => setForm({ ...form, [name]: event.target.value }) });
  return (
    <form className="history-search" role="search" onSubmit={submit}>
      <Field label="Search" placeholder="Title or brief" maxLength={200} {...field("text")} className="history-text" />
      <Field label="Moved from" type="date" hint="Whole days, in UTC" {...field("from")} />
      <Field label="Until" type="date" {...field("to")} {...(error === undefined ? {} : { error })} />
      <Button type="submit" variant="primary">Search</Button>
    </form>
  );
}

/** A `list_items` row as core stored it: a queue item, whose state is tagged by `state`. */
interface ListedRow { readonly id: string; readonly title: string; readonly state: { readonly state: string } | string; readonly updated_at: string }

function Results({ access, query, selected, onSelect }: { readonly access: ArchonAccess; readonly query: ItemSearch; readonly selected: string | undefined; readonly onSelect: (item: string) => void }) {
  const app = useApp();
  const { conn, ready } = access;
  const signal = useSettled(useActivity(conn), SETTLE_MS);
  const read = useMemo(() => ready ? () => app.manager.searchItems(conn, query) : null, [app, conn, query, ready]);
  const { failure, reload } = useStoreRead(read, signal);
  const stored = useSelector(useMemo(() => selectSearch(conn, searchKey(query)), [conn, query]));
  const items = useSelector(useMemo(() => selectItems(conn), [conn]));
  if (stored === undefined) {
    if (failure !== null) return <ErrorState title="The search failed" message={failure.message} onRetry={reload} />;
    return <div className="history-results" aria-busy="true" aria-label="Searching"><Skeleton height="2.5rem" /><Skeleton height="2.5rem" /><Skeleton height="2.5rem" /></div>;
  }
  const rows = stored.value.items as unknown as readonly ListedRow[];
  if (rows.length === 0) return <EmptyState title="No items moved then">Widen the range or search for other words.</EmptyState>;
  return (
    <div className="history-results">
      <p className="dag-muted">{rows.length === SEARCH_LIMIT ? `The newest ${SEARCH_LIMIT} items` : `${rows.length} items`}{failure === null ? "" : ` · the last search failed: ${failure.message}`}</p>
      <ul className="history-list" aria-label="Items">
        {rows.map((row) => {
          // Core's item, when it holds one, is newer than the search that listed it.
          const live = items[row.id];
          const kind = live?.state ?? (typeof row.state === "string" ? row.state : row.state.state);
          const title = typeof live?.summary.title === "string" ? live.summary.title : row.title;
          const moved = typeof live?.summary.updated_at === "string" ? live.summary.updated_at : row.updated_at;
          invariant(isStateKind(kind), `listed item ${row.id} has a known state`);
          const pill = statePill(kind, false);
          return (
            <li key={row.id}>
              <button type="button" className="dag-row history-row" aria-pressed={row.id === selected} onClick={() => onSelect(row.id)}>
                <span className="dag-row-title">{title}</span>
                <StatusPill status={pill.status} label={pill.label} />
                <span className="dag-reason">Moved {dateTime(moved)}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/** The selected item's title as core holds it now: its summary, else the search row that listed it. */
function useTitle(conn: string, query: ItemSearch, item: string | undefined): string {
  const summary = useSelector(useMemo(() => selectItem(conn, item ?? ""), [conn, item]));
  const stored = useSelector(useMemo(() => selectSearch(conn, searchKey(query)), [conn, query]));
  if (item === undefined) return "";
  if (typeof summary?.summary.title === "string") return summary.summary.title;
  const row = (stored?.value.items as readonly ListedRow[] | undefined)?.find((listed) => listed.id === item);
  return row?.title ?? item;
}

function Explorer({ info }: { readonly info: ConnectionInfo }) {
  const access = useArchonAccess(info);
  const [initial] = useState(() => defaultForm(Date.now()));
  const [query, setQuery] = useState<ItemSearch>(() => queryOf(initial));
  // Only the selection's id is kept; everything shown about it is read from core.
  const [picked, setPicked] = useState<string>();
  const title = useTitle(access.conn, query, picked);
  const onSelect = useCallback((item: string) => setPicked(item), []);
  if (access.ready && !access.has("item_filters")) {
    return <EmptyState title="No history search on this Archon">This archond cannot search items by time or text. Update it to 1.5 or later to explore what moved.</EmptyState>;
  }
  return (
    <div className="history">
      <SearchBar initial={initial} onSearch={setQuery} />
      <div className="history-body">
        <Results access={access} query={query} selected={picked} onSelect={onSelect} />
        <aside className="dag-panel history-panel" aria-label="Item history">
          {picked === undefined ? <p className="dag-muted">Pick an item to see every state it moved through, and why.</p>
            : !access.has("item_history") ? <p className="dag-muted">This archond does not record item histories.</p>
              : <Timeline key={picked} access={access} item={picked} title={title} />}
        </aside>
      </div>
    </div>
  );
}

export default function HistoryScreen() {
  const info = useArchonConnection();
  return info ? <Explorer info={info} /> : <NotConnected />;
}
