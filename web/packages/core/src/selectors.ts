// The exported selectors (CONTRACTS.md §2.5): features read the store only
// through these, so its layout stays core's own. Each returns a reference
// held by the state, never a fresh copy, so useSelector stays quiet.

import type { aether, Capabilities, ConnectionInfo, ConnId } from "@aether-ui/protocol";
import type { ArchonGraph, ArchonItem, Report, State, StreamState, Submission, TranscriptState } from "./store.ts";

type Selector<T> = (state: State) => T;

const NONE: readonly never[] = Object.freeze([]);
const EMPTY: Readonly<Record<string, never>> = Object.freeze({});

export const selectConnections: Selector<Readonly<Record<ConnId, ConnectionInfo>>> = (s) => s.connections;
export const selectConnection = (conn: ConnId): Selector<ConnectionInfo | undefined> => (s) => s.connections[conn];
export const selectCapabilities: Selector<Capabilities> = (s) => s.capabilities;
export const selectReview: Selector<readonly Submission[]> = (s) => s.review;
export const selectReports: Selector<readonly Report[]> = (s) => s.reports;

// Aether: agents, attention, events, transcripts, live streams, outbox.
export const selectAgents = (conn: ConnId): Selector<Readonly<Record<string, aether.Agent>>> => (s) => s.aether[conn]?.agents ?? EMPTY;
export const selectAgent = (conn: ConnId, agent: string): Selector<aether.Agent | undefined> => (s) => s.aether[conn]?.agents[agent];
export const selectRoots = (conn: ConnId): Selector<readonly string[]> => (s) => s.aether[conn]?.roots ?? NONE;
export const selectAttention = (conn: ConnId): Selector<aether.Attention | null> => (s) => s.aether[conn]?.attention ?? null;
export const selectEvents = (conn: ConnId): Selector<readonly aether.Event[]> => (s) => s.aether[conn]?.events ?? NONE;
export const selectTranscript = (conn: ConnId, agent: string): Selector<TranscriptState | undefined> => (s) => s.aether[conn]?.transcripts[agent];
export const selectStreams = (conn: ConnId): Selector<Readonly<Record<string, StreamState>>> => (s) => s.aether[conn]?.streams ?? EMPTY;
export const selectOutbox = (conn: ConnId): Selector<readonly Submission[]> => (s) => s.aether[conn]?.outbox ?? s.archon[conn]?.outbox ?? NONE;

// Archon: items with their live state, the task graph, item detail, the feed.
export const selectItems = (conn: ConnId): Selector<Readonly<Record<string, ArchonItem>>> => (s) => s.archon[conn]?.items ?? EMPTY;
export const selectItem = (conn: ConnId, item: string): Selector<ArchonItem | undefined> => (s) => s.archon[conn]?.items[item];
export const selectItemDetail = (conn: ConnId, item: string): Selector<unknown> => (s) => s.archon[conn]?.details[item];
export const selectTaskGraph = (conn: ConnId): Selector<ArchonGraph | null> => (s) => s.archon[conn]?.graph ?? null;
export const selectFeed = (conn: ConnId): Selector<readonly unknown[]> => (s) => s.archon[conn]?.feed ?? NONE;
