// Messages sent from the composer and not yet delivered, per connection and agent: kept while the
// app is open so a visit back still shows them, and settled against the store's rows and events.

import type { aether, ConnId } from "@aether-ui/protocol";
import { useCallback, useEffect, useMemo, useRef, useSyncExternalStore } from "react";
import { invariant } from "../fleet/invariant.ts";
import type { OnSend } from "./Composer.tsx";
import { settleQueued, type ConversationRow, type QueuedMessage } from "./rows.ts";

const NO_QUEUED: readonly QueuedMessage[] = [];

/** The queued messages under `<conn>/<agent>`; a change tells the key's subscribers. */
export class QueuedStore {
  private readonly lists = new Map<string, readonly QueuedMessage[]>();
  private readonly listeners = new Map<string, Set<() => void>>();

  get(key: string): readonly QueuedMessage[] {
    return this.lists.get(key) ?? NO_QUEUED;
  }

  set(key: string, next: readonly QueuedMessage[]): void {
    invariant(key !== "", "queued messages belong to a connection and agent");
    if (next.length === 0) this.lists.delete(key);
    else this.lists.set(key, next);
    for (const listener of this.listeners.get(key) ?? []) listener();
  }

  subscribe(key: string, listener: () => void): () => void {
    const set = this.listeners.get(key) ?? new Set<() => void>();
    set.add(listener);
    this.listeners.set(key, set);
    return () => {
      set.delete(listener);
      if (set.size === 0) this.listeners.delete(key);
    };
  }
}

export const queuedStore = new QueuedStore();

/**
 * The agent's queued messages, those sent here and not yet delivered. `begin` notes the transcript's
 * watermark when a send starts, so a delivery that lands before the daemon's reply is not shown as
 * queued again; the message joins the list once the daemon accepted it. What settles is written back:
 * a learned inbox seq is kept after the recent events forget the delivery, and a delivered message
 * leaves for good.
 */
export function useQueuedMessages(
  conn: ConnId, agent: string, rows: readonly ConversationRow[], events: readonly aether.Event[], inboxCursor: number, store: QueuedStore = queuedStore,
): { queued: readonly QueuedMessage[]; begin: OnSend } {
  invariant(conn !== "" && agent !== "", "queued messages name their connection and agent");
  const key = `${conn}/${agent}`;
  const sent = useSyncExternalStore(
    useCallback((listener: () => void) => store.subscribe(key, listener), [store, key]),
    useCallback(() => store.get(key), [store, key]),
  );
  const queued = useMemo(() => settleQueued(sent, rows, events, agent, inboxCursor), [sent, rows, events, agent, inboxCursor]);
  const newestSeq = useRef(0);
  useEffect(() => {
    newestSeq.current = rows.reduce((top, row) => Math.max(top, row.seq ?? 0), 0);
  }, [rows]);
  useEffect(() => {
    const unchanged = queued.length === sent.length && queued.every((message, index) => message === sent[index]);
    if (!unchanged) store.set(key, queued);
  }, [store, key, queued, sent]);
  const begin = useCallback<OnSend>((text) => {
    invariant(text.trim() !== "", "a queued message has text");
    const message: QueuedMessage = { text, at: new Date().toISOString(), afterSeq: newestSeq.current, inboxSeq: null };
    return (accepted) => {
      if (!accepted) return;
      store.set(key, [...store.get(key), message]);
    };
  }, [store, key]);
  return { queued, begin };
}
