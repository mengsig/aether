// An agent's transcript: committed entries and live tails, all read from core's store and drawn
// through a measured virtual list as the view mode shapes them, with find over what is drawn.
// Opening it asks core to watch the agent, which subscribes to its deltas and pages its transcript in
// with `get_transcript_page`; a delta gap rereads from the last entry.

import { selectAgent, selectConnection, selectEvents, selectStreams, selectTranscript, useApp, useSelector, type State } from "@aether-ui/core";
import type { aether, ConnId, ConnectionInfo } from "@aether-ui/protocol";
import { Button, EmptyState, Skeleton } from "@aether-ui/ui";
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { invariant } from "../fleet/invariant.ts";
import "./conversation.css";
import "./markdown.css";
import { MeasuredList, type ListHandle } from "./MeasuredList.tsx";
import { AgentNameContext, estimateRow, MessageView, type MessageContext } from "./Message.tsx";
import type { ModeRules } from "./modes.ts";
import { entryRows, invocationMeta, liveRows, pairTools, toolActivity, type ConversationRow, type ToolActivity } from "./rows.ts";
import { SearchBar, useTranscriptSearch } from "./SearchBar.tsx";
import { ToggleContext, ToggleSet } from "./toggles.ts";
import { ToolActivityContext } from "./ToolCall.tsx";
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

export interface ConversationRows {
  readonly rows: readonly ConversationRow[];
  readonly loaded: boolean;
  /** When each tool call ran, by tool_use_id, from the agent's events. */
  readonly activity: ReadonlyMap<string, ToolActivity>;
}

/** Every row the screen shows, derived from the store: committed entries, then uncommitted live tails. */
export function useConversationRows(conn: ConnId, agent: string): ConversationRows {
  const transcript = useSelector(useMemo(() => selectTranscript(conn, agent), [conn, agent]));
  const streams = useSelector(useMemo(() => selectStreams(conn), [conn]));
  const events = useSelector(useMemo(() => selectEvents(conn), [conn]));
  const entries = transcript?.entries ?? NO_ENTRIES;
  const pairing = useMemo(() => pairTools(entries), [entries]);
  const meta = useMemo(() => invocationMeta(events, agent), [events, agent]);
  const activity = useMemo(() => toolActivity(events, agent), [events, agent]);
  const committed = useMemo(() => entryRows(entries, pairing, meta), [entries, pairing, meta]);
  const live = useMemo(() => liveRows(streams, agent, pairing.committed), [streams, agent, pairing]);
  const rows = useMemo(() => (live.length === 0 ? committed : [...committed, ...live]), [committed, live]);
  invariant(rows.length >= committed.length, "live tails only add rows");
  return { rows, loaded: transcript !== undefined, activity };
}

/** The rows a mode draws, recomputed per row only when a row or the mode changes. */
function useViewRows(conn: ConnId, agent: string, rows: readonly ConversationRow[], rules: ModeRules | null): { views: readonly ViewRow[]; context: MessageContext; name: string } {
  const self = useSelector(useMemo(() => selectAgent(conn, agent), [conn, agent]));
  const parent = self?.parent ?? null;
  const taskRow = taskRowKey(rows, parent !== null);
  // One cache per mode: a row's view is computed once while the mode holds.
  const mode = useMemo(() => ({ options: { rules, taskRow }, cache: new WeakMap<ConversationRow, ViewRow | null>() }), [rules, taskRow]);
  const views = useMemo(() => viewRows(rows, mode.options, mode.cache), [rows, mode]);
  const context = useMemo(() => ({ conn, parent }), [conn, parent]);
  return { views, context, name: self?.name ?? "Agent" };
}

/** A quiet line over the transcript while the connection is down; what arrived stays readable. */
function ConnectionBanner({ info }: { readonly info: ConnectionInfo | undefined }) {
  if (info?.state === "ready") return null;
  const label = info?.label ?? "the daemon";
  const text = info?.state === "pairing" ? `${label} asks to pair this device again.`
    : info?.state === "closed" || info === undefined ? `Disconnected from ${label}. New messages appear once it is back.`
      : `Connection to ${label} lost. Reconnecting…`;
  return <p className="conv-banner" role="status" data-testid="conv-connection" data-state={info?.state ?? "closed"}>{text}</p>;
}

/** Find over the drawn rows: their keys are the list's, so a match is painted in and scrolled to its row. */
function useFind(views: readonly ViewRow[], layout: string, list: React.RefObject<ListHandle | null>, stage: React.RefObject<HTMLDivElement | null>, toggles: ToggleSet) {
  const rows = useMemo(() => views.map((view) => ({ ...view.row, key: `${layout}|${view.row.key}`, parts: view.parts.map((part) => part.part) })), [views, layout]);
  const search = useTranscriptSearch({ rows, list, stage, toggles });
  const [focusFind, setFocusFind] = useState(0);
  const open = () => {
    search.show();
    setFocusFind((n) => n + 1);
  };
  const close = () => {
    search.hide();
    stage.current?.querySelector<HTMLElement>(".conv-list")?.focus();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key.toLowerCase() !== "f" || !(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) return;
    event.preventDefault();
    open();
  };
  return { search, focusFind, open, close, onKeyDown };
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
  const { rows, loaded, activity } = useConversationRows(conn, agent);
  const { views, context, name } = useViewRows(conn, agent, rows, rules);
  const [toggles] = useState(() => new ToggleSet());
  const list = useRef<ListHandle>(null);
  const stage = useRef<HTMLDivElement>(null);
  const find = useFind(views, layout, list, stage, toggles);
  const currentRow = find.search.currentRow;
  const rowKey = useCallback((index: number) => `${layout}|${views[index]?.row.key ?? String(index)}`, [views, layout]);
  const position = useCallback((index: number) => positionOf(views[index]), [views]);
  const estimate = useCallback((index: number) => {
    const row = views[index];
    return row === undefined ? 0 : cachedEstimate(row);
  }, [views]);
  const renderRow = useCallback((index: number) => {
    const row = views[index];
    invariant(row !== undefined, `the list asked for row ${index} of ${views.length}`);
    return <MessageView view={row} context={context} rowKey={`${layout}|${row.row.key}`} current={index === currentRow} />;
  }, [views, context, layout, currentRow]);

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
  if (rows.length === 0) {
    return (
      <>
        <ConnectionBanner info={info} />
        <EmptyState className="conv-empty" title="No messages yet">
          The agent's replies, its tool calls and the messages it receives appear here as it works. Write to it in the box below.
        </EmptyState>
      </>
    );
  }
  const hidden = rows.length - views.length;
  return (
    <section className="conv" aria-label="Conversation" data-density={rules?.density ?? "comfortable"}>
      <div className="conv-toolbar" onKeyDown={find.onKeyDown}>
        <p className="conv-count">
          {views.length.toLocaleString("en")} messages
          {hidden > 0 ? <span className="conv-note"> · {hidden.toLocaleString("en")} hidden by this view</span> : null}
        </p>
        {find.search.open ? <SearchBar key={find.focusFind} search={find.search} onClose={find.close} />
          : <Button variant="quiet" size="sm" className="conv-find" aria-keyshortcuts="Control+F Meta+F" title="Find in the transcript (Ctrl+F or ⌘F)" onClick={find.open}>Find</Button>}
      </div>
      <ConnectionBanner info={info} />
      <div className="conv-stage" ref={stage} onKeyDown={find.onKeyDown}>
        <ToggleContext.Provider value={toggles}>
          <ToolActivityContext.Provider value={activity}>
            <AgentNameContext.Provider value={name}>
              <MeasuredList ref={list} label="Messages" count={views.length} rowKey={rowKey} estimate={estimate} renderRow={renderRow} follow layout={layout} position={position} />
            </AgentNameContext.Provider>
          </ToolActivityContext.Provider>
        </ToggleContext.Provider>
      </div>
    </section>
  );
}
