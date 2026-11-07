// An agent's transcript: committed entries and live tails, all read from core's store and drawn
// through a measured virtual list, with find, a jump to the newest message and the composer below.
// Opening it asks core to watch the agent, which subscribes to its deltas and pages its transcript in.

import { selectAgent, selectConnection, selectEvents, selectStreams, selectTranscript, useApp, useSelector, type State } from "@aether-ui/core";
import type { aether, ConnId, ConnectionInfo } from "@aether-ui/protocol";
import { Button, EmptyState } from "@aether-ui/ui";
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { invariant } from "../fleet/invariant.ts";
import { Composer } from "./Composer.tsx";
import "./conversation.css";
import "./markdown.css";
import type { ListHandle } from "./MeasuredList.tsx";
import { AgentNameContext } from "./Message.tsx";
import { useQueuedMessages } from "./queued.ts";
import { entryRows, invocationMeta, liveRows, pairTools, queuedRows, rowCounts, toolActivity, type ConversationRow, type ToolActivity } from "./rows.ts";
import { SearchBar, useTranscriptSearch } from "./SearchBar.tsx";
import { ToggleContext, ToggleSet } from "./toggles.ts";
import { ToolActivityContext } from "./ToolCall.tsx";
import { TranscriptList, TranscriptSkeleton } from "./TranscriptList.tsx";

const NO_ENTRIES: readonly aether.TranscriptEntry[] = Object.freeze([]);

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
  /** How many rows are committed messages; the rest are live tails at the end. */
  readonly committed: number;
  readonly activity: ReadonlyMap<string, ToolActivity>;
  /** The connection's recent durable events, which also tell when a queued message was consumed. */
  readonly events: readonly aether.Event[];
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
  return { rows, loaded: transcript !== undefined, committed: committed.length, activity, events };
}

const counted = (n: number, noun: string): string => `${n.toLocaleString("en")} ${noun}${n === 1 ? "" : "s"}`;

/** A quiet line over the transcript while the connection is down; what arrived stays readable. */
function ConnectionBanner({ info }: { readonly info: ConnectionInfo | undefined }) {
  if (info?.state === "ready") return null;
  const label = info?.label ?? "the daemon";
  const text = info?.state === "pairing" ? `${label} asks to pair this device again.`
    : info?.state === "closed" || info === undefined ? `Disconnected from ${label}. New messages appear once it is back.`
      : `Connection to ${label} lost. Reconnecting…`;
  return <p className="conv-banner" role="status" data-testid="conv-connection" data-state={info?.state ?? "closed"}>{text}</p>;
}

function TranscriptBody({ loaded, info, rows, children }: { readonly loaded: boolean; readonly info: ConnectionInfo | undefined; readonly rows: number; readonly children: ReactNode }) {
  if (!loaded && info?.state !== "ready") {
    return <EmptyState title="Not connected">The conversation loads once {info?.label ?? "the connection"} is connected.</EmptyState>;
  }
  if (!loaded) return <TranscriptSkeleton />;
  if (rows === 0) {
    return (
      <EmptyState className="conv-empty" title="No messages yet">
        The agent's replies, its tool calls and the messages it receives appear here as it works. Write to it in the box below.
      </EmptyState>
    );
  }
  return children;
}

export function Conversation({ conn, agent }: { readonly conn: ConnId; readonly agent: string }) {
  invariant(conn !== "" && agent !== "", "a conversation names its connection and agent");
  useWatchAgent(conn, agent);
  const info = useSelector(useMemo(() => selectConnection(conn), [conn]));
  const row = useSelector(useMemo(() => selectAgent(conn, agent), [conn, agent]));
  const name = row?.name ?? "Agent";
  const { rows, loaded, committed, activity, events } = useConversationRows(conn, agent);
  const [toggles] = useState(() => new ToggleSet());
  const list = useRef<ListHandle>(null);
  const stage = useRef<HTMLDivElement>(null);
  const sent = useQueuedMessages(conn, agent, rows, events, row?.inbox_cursor ?? 0);
  const shown = useMemo(() => (sent.queued.length === 0 ? rows : [...rows, ...queuedRows(sent.queued)]), [rows, sent.queued]);
  const search = useTranscriptSearch({ rows: shown, list, stage, toggles });
  const [focusFind, setFocusFind] = useState(0);
  const counts = useMemo(() => rowCounts(shown), [shown]);
  const openFind = () => {
    search.show();
    setFocusFind((n) => n + 1);
  };
  const closeFind = () => {
    search.hide();
    stage.current?.querySelector<HTMLElement>(".conv-list")?.focus();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key.toLowerCase() !== "f" || !(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) return;
    event.preventDefault();
    openFind();
  };
  return (
    <section className="conv" aria-label="Conversation">
      <header className="conv-bar" onKeyDown={onKeyDown}>
        <p className="conv-count">{counted(counts.messages, "message")} · {counted(counts.tools, "tool call")}</p>
        {search.open ? <SearchBar key={focusFind} search={search} onClose={closeFind} />
          : <Button variant="quiet" size="sm" className="conv-find" aria-keyshortcuts="Control+F Meta+F" title="Find in the transcript (Ctrl+F or ⌘F)" onClick={openFind}>Find</Button>}
      </header>
      {loaded ? <ConnectionBanner info={info} /> : null}
      <div className="conv-stage" ref={stage} onKeyDown={onKeyDown}>
        <ToggleContext.Provider value={toggles}>
          <ToolActivityContext.Provider value={activity}>
            <AgentNameContext.Provider value={name}>
              <TranscriptBody loaded={loaded} info={info} rows={shown.length}>
                <TranscriptList conn={conn} agent={agent} rows={shown} committed={committed} currentRow={search.currentRow} handle={list} />
              </TranscriptBody>
            </AgentNameContext.Provider>
          </ToolActivityContext.Provider>
        </ToggleContext.Provider>
      </div>
      <Composer conn={conn} agent={agent} onSend={(text) => { const settle = sent.begin(text); return (accepted) => { settle(accepted); if (accepted) list.current?.scrollToEnd(); }; }} />
    </section>
  );
}
