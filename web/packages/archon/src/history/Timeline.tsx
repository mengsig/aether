// One item's state transitions as `item_history` recorded them, oldest first: from → to, why, who
// moved it and how long it stayed. Core stores the history; while it is shown core keeps it current.

import { routeHref, selectItem, selectItemHistory, useApp, useSelector } from "@aether-ui/core";
import { ErrorState, Skeleton, StatusPill } from "@aether-ui/ui";
import { useEffect, useMemo } from "react";
import { useStoreRead } from "../initiatives/read.ts";
import type { ArchonAccess } from "../queue/archon.ts";
import { dateTime, invariant } from "../queue/model.ts";
import { useNow } from "../queue/QueueScreen.tsx";
import { duration, timeline, type Transition } from "./model.ts";

function Rows({ transitions }: { readonly transitions: readonly Transition[] }) {
  const now = useNow(30_000);
  const rows = useMemo(() => timeline(transitions, now), [transitions, now]);
  if (rows.length === 0) return <p className="dag-muted">Archon recorded no transitions for this item. Open the item to inspect its current state.</p>;
  return (
    <ol className="history-timeline" aria-label="Transitions">
      {rows.map((row, index) => (
        <li key={row.seq} className="history-step" data-seq={row.seq}>
          <span className="history-dot" data-status={row.to.status} aria-hidden="true" />
          <div className="history-move">
            {row.from === null ? <span className="dag-muted">Created</span> : <StatusPill status={row.from.status} label={row.from.label} />}
            <span className="history-arrow" aria-label="to">→</span>
            <StatusPill status={row.to.status} label={row.to.label} />
          </div>
          {row.reason === null ? null : <p className="history-reason">{row.reason}</p>}
          <p className="dag-muted history-meta">
            <time dateTime={row.at}>{dateTime(row.at)}</time> · by {row.by}{index === rows.length - 1 && ["succeeded", "failed", "cancelled"].includes(transitions[index]?.to ?? "") ? null : ` · ${index === rows.length - 1 ? `${duration(row.heldMs)} so far` : `held ${duration(row.heldMs)}`}`}
          </p>
        </li>
      ))}
    </ol>
  );
}

export function Timeline({ access, item, title }: { readonly access: ArchonAccess; readonly item: string; readonly title: string }) {
  invariant(item.length > 0, "a timeline is of an item");
  const app = useApp();
  const { conn, live, ready } = access;
  useEffect(() => {
    // Shown, core rereads the item and the history it holds on each of its changes (§5.3).
    if (!live || !ready) return;
    app.manager.showItems(conn, [item]);
    return () => app.manager.showItems(conn, []);
  }, [app, conn, item, live, ready]);
  const read = useMemo(() => ready ? () => app.manager.readHistory(conn, item) : null, [app, conn, item, ready]);
  const { failure, reload } = useStoreRead(read, "");
  const stored = useSelector(useMemo(() => selectItemHistory(conn, item), [conn, item]));
  const summary = useSelector(useMemo(() => selectItem(conn, item), [conn, item]));
  const transitions = (stored?.value.transitions ?? null) as readonly Transition[] | null;
  return (
    <section className="history-detail" aria-label={`History of ${title}`} data-item={item}>
      <header className="history-detail-header">
        <h2 className="dag-title">{title}</h2>
        <a className="ui-link" href={routeHref("#/archon/items/:item", { item })}>Open the item</a>
      </header>
      {summary === undefined ? null : <p className="dag-muted">Now {summary.state}{summary.stage ? ` · stage ${summary.stage}` : ""}</p>}
      {failure !== null && transitions === null ? <ErrorState title="The history could not be read" message={failure.message} onRetry={reload} /> : null}
      {failure !== null && transitions !== null ? <p className="dag-warning" role="status">Showing the last read: {failure.message}</p> : null}
      {transitions === null && failure === null ? <div aria-busy="true" aria-label="Loading the history"><Skeleton height="3rem" /><Skeleton height="3rem" /><Skeleton height="3rem" /></div> : null}
      {transitions === null ? null : <Rows transitions={transitions} />}
    </section>
  );
}
