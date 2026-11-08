// The exported selectors (CONTRACTS.md §2.5). Each returns a reference held
// by the state, never a fresh copy, so useSelector stays quiet.

import type { aether, Capabilities, ConnectionInfo, ConnId } from "@aether-ui/protocol";
import type { NoesisCache } from "./noesis-cache.ts";
import { EMPTY_NOESIS } from "./noesis-cache.ts";
import type { ReadEntry } from "./reads.ts";
import type { ArchonGraph, ArchonItem, ArchonPlan, Report, Stamped, State, StreamState, Submission, TranscriptState } from "./store.ts";

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
export const selectItemDetail = (conn: ConnId, item: string): Selector<Stamped | undefined> => (s) => s.archon[conn]?.details[item];
export const selectItemMissing = (conn: ConnId, item: string): Selector<boolean> => (s) => s.archon[conn]?.missingItems.includes(item) ?? false;
export const selectItemLinks = (conn: ConnId, item: string): Selector<Stamped | undefined> => (s) => s.archon[conn]?.links[item];
export const selectRunPlan = (conn: ConnId, run: string): Selector<ArchonPlan | undefined> => (s) => s.archon[conn]?.plans[run];
export const selectTaskGraph = (conn: ConnId): Selector<ArchonGraph | null> => (s) => s.archon[conn]?.graph ?? null;
export const selectItemHistory = (conn: ConnId, item: string): Selector<Stamped | undefined> => (s) => s.archon[conn]?.histories[item];
export const selectInitiative = (conn: ConnId, initiative: string): Selector<Stamped | undefined> => (s) => s.archon[conn]?.initiatives[initiative];
export const selectSearch = (conn: ConnId, key: string): Selector<Stamped | undefined> => (s) => s.archon[conn]?.searches[key];
export const selectFeed = (conn: ConnId): Selector<readonly unknown[]> => (s) => s.archon[conn]?.feed ?? NONE;

// Noesis: the status core read at ready, and the learning entities (noesis-cache.ts).
export const selectNoesisStatus = (conn: ConnId): Selector<Readonly<Record<string, unknown>> | null> => (s) => s.aether[conn]?.noesis ?? null;
export const selectLearn = (conn: ConnId): Selector<NoesisCache> => (s) => s.aether[conn]?.learn ?? EMPTY_NOESIS;

// Rereads stored whole (reads.ts), and every submission whose outcome is not known.
export const selectRead = (conn: ConnId, key: string): Selector<ReadEntry | undefined> => (s) => s.reads[conn]?.[key];
