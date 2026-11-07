// An agent's transcript: committed entries and live tails, all read from core's store and drawn
// through a measured virtual list. Opening it asks core to watch the agent, which subscribes to its
// deltas and pages its transcript in with `get_transcript_page`; a delta gap rereads from the last entry.

import { selectConnection, selectEvents, selectStreams, selectTranscript, useApp, useSelector, type State } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { EmptyState, Skeleton } from "@aether-ui/ui";
import { useCallback, useEffect, useMemo } from "react";
import { invariant } from "../fleet/invariant.ts";
import "./conversation.css";
import { MeasuredList } from "./MeasuredList.tsx";
import { estimateRow, MessageView } from "./Message.tsx";
import { entryRows, invocationMeta, liveRows, pairTools, type ConversationRow } from "./rows.ts";

const NO_ENTRIES: readonly aether.TranscriptEntry[] = Object.freeze([]);
const estimates = new WeakMap<ConversationRow, number>();

function cachedEstimate(row: ConversationRow): number {
  let height = estimates.get(row);
  if (height === undefined) {
    height = estimateRow(row);
    estimates.set(row, height);
  }
  return height;
}

/** Asks core to stream this agent while it is on screen; core resumes the watch after reconnects. */
function useWatchAgent(conn: ConnId, agent: string): void {
  const app = useApp();
  const known = useSelector(useCallback((state: State) => state.connections[conn]?.daemon === "aether", [conn]));
  useEffect(() => {
    if (!known) return undefined;
    const watch = (agents: readonly string[]) => app.manager.watchAgents(conn, agents).catch((error: unknown) => {
      app.store.report(`Could not follow ${agent}: ${error instanceof Error ? error.message : String(error)}`, conn);
    });
    void watch([agent]);
    return () => void watch([]);
  }, [app, conn, agent, known]);
}

/** Every row the screen shows, derived from the store: committed entries, then uncommitted live tails. */
export function useConversationRows(conn: ConnId, agent: string): { rows: readonly ConversationRow[]; loaded: boolean } {
  const transcript = useSelector(useMemo(() => selectTranscript(conn, agent), [conn, agent]));
  const streams = useSelector(useMemo(() => selectStreams(conn), [conn]));
  const events = useSelector(useMemo(() => selectEvents(conn), [conn]));
  const entries = transcript?.entries ?? NO_ENTRIES;
  const pairing = useMemo(() => pairTools(entries), [entries]);
  const meta = useMemo(() => invocationMeta(events, agent), [events, agent]);
  const committed = useMemo(() => entryRows(entries, pairing, meta), [entries, pairing, meta]);
  const live = useMemo(() => liveRows(streams, agent, pairing.committed), [streams, agent, pairing]);
  const rows = useMemo(() => (live.length === 0 ? committed : [...committed, ...live]), [committed, live]);
  invariant(rows.length >= committed.length, "live tails only add rows");
  return { rows, loaded: transcript !== undefined };
}

export function Conversation({ conn, agent }: { readonly conn: ConnId; readonly agent: string }) {
  invariant(conn !== "" && agent !== "", "a conversation names its connection and agent");
  useWatchAgent(conn, agent);
  const info = useSelector(useMemo(() => selectConnection(conn), [conn]));
  const { rows, loaded } = useConversationRows(conn, agent);
  const rowKey = useCallback((index: number) => rows[index]?.key ?? String(index), [rows]);
  const estimate = useCallback((index: number) => {
    const row = rows[index];
    return row === undefined ? 0 : cachedEstimate(row);
  }, [rows]);
  const renderRow = useCallback((index: number) => {
    const row = rows[index];
    invariant(row !== undefined, `the list asked for row ${index} of ${rows.length}`);
    return <MessageView row={row} />;
  }, [rows]);

  if (!loaded && info?.state !== "ready") {
    return <EmptyState title="Not connected">The conversation loads once {info?.label ?? conn} is connected.</EmptyState>;
  }
  if (!loaded) {
    return (
      <div className="ui-loading conv-loading" aria-busy="true" aria-label="Reading the transcript">
        <Skeleton height="4rem" /><Skeleton height="6rem" /><Skeleton height="3rem" />
      </div>
    );
  }
  if (rows.length === 0) return <EmptyState title="No messages yet">The conversation appears here as the agent works.</EmptyState>;
  return (
    <section className="conv" aria-label="Conversation">
      <p className="conv-count">{rows.length.toLocaleString("en")} messages</p>
      <MeasuredList label="Messages" count={rows.length} rowKey={rowKey} estimate={estimate} renderRow={renderRow} follow />
    </section>
  );
}
