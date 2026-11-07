// An agent's transcript: committed entries and live tails, all read from core's store and drawn
// through a measured virtual list. Opening it asks core to watch the agent, which subscribes to its
// deltas and pages its transcript in with `get_transcript_page`; a delta gap rereads from the last entry.

import { selectAgent, selectAgents, selectConnection, selectEvents, selectStreams, selectTranscript, useApp, useReread, useSelector, type State } from "@aether-ui/core";
import { aether, type ConnId } from "@aether-ui/protocol";
import { EmptyState, Skeleton } from "@aether-ui/ui";
import { useCallback, useEffect, useMemo } from "react";
import { invariant } from "../fleet/invariant.ts";
import "./conversation.css";
import { MeasuredList } from "./MeasuredList.tsx";
import { estimateRow, MessageView, type MessageContext } from "./Message.tsx";
import type { ModeRules } from "./modes.ts";
import { deliveries, highestSeq, type Deliveries } from "./inbox.ts";
import { entryRows, invocationMeta, liveRows, pairTools, type ConversationRow } from "./rows.ts";
import { taskRowKey, viewRows, type ViewRow } from "./view.ts";

const NO_ENTRIES: readonly aether.TranscriptEntry[] = Object.freeze([]);
const estimates = new WeakMap<ViewRow, number>();

function cachedEstimate(row: ViewRow): number {
  let height = estimates.get(row);
  if (height === undefined) {
    height = estimateRow(row);
    estimates.set(row, height);
  }
  return height;
}

/** Where a row sits in the transcript, shared by every mode: its seq, live tails last. */
const positionOf = (row: ViewRow | undefined): number => row?.row.seq ?? Number.MAX_SAFE_INTEGER;

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

/** The highest inbox seq the loaded user turns name; a new delivery raises it. */
function deliveredUpTo(entries: readonly aether.TranscriptEntry[]): number {
  let highest = 0;
  for (const entry of entries) {
    if (entry.message.role !== "user") continue;
    for (const block of entry.message.content) if (block.type === "text") highest = Math.max(highest, highestSeq(block.text));
  }
  return highest;
}

/** The agent's inbox records, reread once a turn names a delivery newer than the last read; null until read. */
function useDeliveries(conn: ConnId, agent: string, entries: readonly aether.TranscriptEntry[]): Deliveries | null {
  const upTo = useMemo(() => deliveredUpTo(entries), [entries]);
  const read = useReread(conn, `conversation:inbox:${agent}`, aether.getInbox({ agent, after: 0 }), ["inbox"], upTo);
  const items = read?.value;
  invariant(items === undefined || items === null || Array.isArray(items), "get_inbox answers with a list of items");
  return useMemo(() => deliveries((items ?? null) as readonly aether.InboxItem[] | null, agent), [items, agent]);
}

/** Every row the screen shows, derived from the store: committed entries, then uncommitted live tails. */
export function useConversationRows(conn: ConnId, agent: string): { rows: readonly ConversationRow[]; loaded: boolean } {
  const transcript = useSelector(useMemo(() => selectTranscript(conn, agent), [conn, agent]));
  const streams = useSelector(useMemo(() => selectStreams(conn), [conn]));
  const events = useSelector(useMemo(() => selectEvents(conn), [conn]));
  const entries = transcript?.entries ?? NO_ENTRIES;
  const pairing = useMemo(() => pairTools(entries), [entries]);
  const meta = useMemo(() => invocationMeta(events, agent), [events, agent]);
  const records = useDeliveries(conn, agent, entries);
  const committed = useMemo(() => entryRows(entries, pairing, meta, records), [entries, pairing, meta, records]);
  const live = useMemo(() => liveRows(streams, agent, pairing.committed), [streams, agent, pairing]);
  const rows = useMemo(() => (live.length === 0 ? committed : [...committed, ...live]), [committed, live]);
  invariant(rows.length >= committed.length, "live tails only add rows");
  return { rows, loaded: transcript !== undefined };
}

/** The rows a mode draws, recomputed per row only when a row or the mode changes. */
function useViewRows(conn: ConnId, agent: string, rows: readonly ConversationRow[], rules: ModeRules | null): { views: readonly ViewRow[]; context: MessageContext } {
  const self = useSelector(useMemo(() => selectAgent(conn, agent), [conn, agent]));
  const agents = useSelector(useMemo(() => selectAgents(conn), [conn]));
  const parent = self?.parent ?? null;
  const taskRow = taskRowKey(rows, parent !== null);
  // One cache per mode: a row's view is computed once while the mode holds.
  const mode = useMemo(() => ({ options: { rules, taskRow }, cache: new WeakMap<ConversationRow, ViewRow | null>() }), [rules, taskRow]);
  const views = useMemo(() => viewRows(rows, mode.options, mode.cache), [rows, mode]);
  const context = useMemo(() => ({ directory: { conn, agents }, parent }), [conn, agents, parent]);
  return { views, context };
}

export interface ConversationProps {
  readonly conn: ConnId;
  readonly agent: string;
  /** The view mode's rules; null (the default) draws every row whole. */
  readonly rules?: ModeRules | null;
  /** Names the mode and Details state; a change keeps the row in view in view. */
  readonly layout?: string;
}

export function Conversation({ conn, agent, rules = null, layout = "standard" }: ConversationProps) {
  invariant(conn !== "" && agent !== "", "a conversation names its connection and agent");
  useWatchAgent(conn, agent);
  const info = useSelector(useMemo(() => selectConnection(conn), [conn]));
  const { rows, loaded } = useConversationRows(conn, agent);
  const { views, context } = useViewRows(conn, agent, rows, rules);
  const rowKey = useCallback((index: number) => `${layout}|${views[index]?.row.key ?? String(index)}`, [views, layout]);
  const position = useCallback((index: number) => positionOf(views[index]), [views]);
  const estimate = useCallback((index: number) => {
    const row = views[index];
    return row === undefined ? 0 : cachedEstimate(row);
  }, [views]);
  const renderRow = useCallback((index: number) => {
    const row = views[index];
    invariant(row !== undefined, `the list asked for row ${index} of ${views.length}`);
    return <MessageView view={row} context={context} />;
  }, [views, context]);

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
  const hidden = rows.length - views.length;
  return (
    <section className="conv" aria-label="Conversation" data-density={rules?.density ?? "comfortable"}>
      <p className="conv-count">
        {views.length.toLocaleString("en")} messages
        {hidden > 0 ? <span className="conv-note"> · {hidden.toLocaleString("en")} hidden by this view</span> : null}
      </p>
      <MeasuredList label="Messages" count={views.length} rowKey={rowKey} estimate={estimate} renderRow={renderRow} follow layout={layout} position={position} />
    </section>
  );
}
