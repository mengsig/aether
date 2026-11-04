// An item's detail. With the change stream, core keeps it current once the item is shown (§5.3); a protocol
// 23 archond has no stream, so the page polls as the retired app did (§5.3.5).

import { selectFeed, selectItem, selectItemDetail, useApp, useSelector } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { useEffect, useMemo, useRef, useState } from "react";
import { useRead, type ArchonAccess } from "../queue/archon.ts";
import { invariant, isStateKind, isTerminal } from "../queue/model.ts";

/** A feed entry naming the item is followed this long after, so a burst rereads once. */
export const FOLLOW_MS = 300;
/** An unfinished item is reread this often while the page is visible. */
export const POLL_MS = 15_000;

export interface ItemDetailRead {
  /** A new object on every reread, live or polled: reads tied to the item (links, artifacts, plans) key on it. */
  readonly detail: archon.ItemDetail | undefined;
  readonly error: string | null;
  readonly reload: () => void;
}

/** The sequence of the newest feed entry that names `item`, or null. */
export function lastMention(feed: readonly unknown[], item: string): number | null {
  invariant(item.length > 0, "a mention names an item");
  for (let index = feed.length - 1; index >= 0; index -= 1) {
    const entry = feed[index] as { seq?: unknown; entry?: { item?: unknown } } | null;
    if (entry?.entry?.item === item && typeof entry.seq === "number") return entry.seq;
  }
  return null;
}

/** Counts rereads for a polled item: after feed mentions, summary updates and the 15 s poll. */
function usePollKey(access: ArchonAccess, item: string, enabled: boolean): number {
  const feed = useSelector(useMemo(() => selectFeed(access.conn), [access.conn]));
  const summary = useSelector(useMemo(() => selectItem(access.conn, item), [access.conn, item]));
  const signal = `${String(lastMention(feed, item))}|${String(summary?.summary.updated_at)}`;
  const kind = summary?.state;
  const finished = kind !== undefined && isStateKind(kind) && isTerminal(kind);
  const [key, setKey] = useState(0);
  const seen = useRef(signal);
  useEffect(() => {
    if (!enabled || seen.current === signal) return;
    seen.current = signal;
    const timer = setTimeout(() => setKey((n) => n + 1), FOLLOW_MS);
    return () => clearTimeout(timer);
  }, [enabled, signal]);
  useEffect(() => {
    if (!enabled || finished) return;
    const timer = setInterval(() => { if (document.visibilityState === "visible") setKey((n) => n + 1); }, POLL_MS);
    return () => clearInterval(timer);
  }, [enabled, finished]);
  return key;
}

export function useItemDetail(access: ArchonAccess, item: string): ItemDetailRead {
  invariant(item.length > 0, "an item page names its item");
  const app = useApp();
  const { conn, live, ready } = access;
  useEffect(() => {
    if (!live || !ready) return;
    app.manager.showItems(conn, [item]);
    return () => app.manager.showItems(conn, []);
  }, [app, conn, item, live, ready]);
  const stored = useSelector(useMemo(() => selectItemDetail(conn, item), [conn, item])) as archon.ItemDetail | undefined;
  const key = usePollKey(access, item, !live);
  const polled = useRead<archon.ItemDetail>(access, live ? null : { cmd: "get_item", item }, ["item_detail"], key);
  if (live) return { detail: stored, error: null, reload: () => undefined };
  return { detail: polled.data, error: polled.error, reload: polled.reload };
}
