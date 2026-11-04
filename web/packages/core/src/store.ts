// The one normalized store (ARCHITECTURE.md §a). Writers see every update at
// once through get(); readers see one committed snapshot per animation frame.

import type { aether, Capabilities, ConnectionInfo, ConnId } from "@aether-ui/protocol";
import { invariant, omit, type Json } from "./guards.ts";

export interface TranscriptState {
  readonly entries: readonly aether.TranscriptEntry[];
  /** The highest seq read from transcript pages; live entries past it leave it alone. */
  readonly lastEntrySeq: number;
}

/** The live text of one invocation, valid only while its deltas stay contiguous. */
export interface StreamState {
  readonly agent: string;
  readonly invocation: string;
  readonly text: string;
  readonly thinking: string;
  /** The last `invocation_seq` seen, or null when the daemon sends none. */
  readonly lastInvocationSeq: number | null;
  /** Deltas are dropped until the transcript reread this stream waits for succeeds (§5.2.4). */
  readonly recovering: boolean;
}

/** A keyed mutation sent and not yet answered. */
export interface Submission {
  readonly conn: ConnId;
  readonly key: string;
  readonly command: Json;
  readonly state: "pending" | "unknown";
  readonly error?: string;
}

export interface AetherConnState {
  readonly lastSeq: number | null;
  readonly agents: Readonly<Record<string, aether.Agent>>;
  readonly roots: readonly string[];
  readonly attention: aether.Attention | null;
  readonly events: readonly aether.Event[];
  readonly transcripts: Readonly<Record<string, TranscriptState>>;
  /** Keyed by invocation id. */
  readonly streams: Readonly<Record<string, StreamState>>;
  readonly noesis: Json | null;
  readonly outbox: readonly Submission[];
  /** Entities an applied event made stale; they survive reconnects until refetched. */
  readonly staleAgents: readonly string[];
  readonly staleAttention: boolean;
}

export interface ArchonItem {
  readonly summary: Json;
  readonly state: string;
  readonly stage?: string;
  readonly waitingReason?: string;
}

export interface ArchonGraph {
  readonly nodes: Readonly<Record<string, Json>>;
  readonly edges: readonly Json[];
  readonly initiatives: readonly Json[];
  readonly asOf: number;
  readonly truncated: boolean;
}

export interface ArchonConnState {
  /** The change cursor (§5.3); never mixed with feedSeq. */
  readonly changeCursor: number | null;
  readonly feedSeq: number | null;
  readonly items: Readonly<Record<string, ArchonItem>>;
  readonly itemsAsOf: number | null;
  readonly graph: ArchonGraph | null;
  readonly details: Readonly<Record<string, Json>>;
  readonly feed: readonly Json[];
  readonly outbox: readonly Submission[];
  /** Refreshes an applied change asked for; they survive reconnects until done. */
  readonly staleSnapshots: boolean;
  readonly staleItems: readonly string[];
}

export interface Report { readonly id: number; readonly conn?: ConnId; readonly message: string; readonly code?: string }

export interface State {
  readonly connections: Readonly<Record<ConnId, ConnectionInfo>>;
  readonly aether: Readonly<Record<ConnId, AetherConnState>>;
  readonly archon: Readonly<Record<ConnId, ArchonConnState>>;
  readonly capabilities: Capabilities;
  /** Submissions whose outcome is unknown after their daemon's identity changed (§5.1). */
  readonly review: readonly Submission[];
  readonly reports: readonly Report[];
}

export const EMPTY_AETHER: AetherConnState = Object.freeze({
  lastSeq: null, agents: {}, roots: [], attention: null, events: [], transcripts: {}, streams: {}, noesis: null, outbox: [],
  staleAgents: [], staleAttention: false,
});

export const EMPTY_ARCHON: ArchonConnState = Object.freeze({
  changeCursor: null, feedSeq: null, items: {}, itemsAsOf: null, graph: null, details: {}, feed: [], outbox: [],
  staleSnapshots: false, staleItems: [],
});

export const INITIAL_STATE: State = Object.freeze({
  connections: {}, aether: {}, archon: {}, capabilities: Object.freeze({}), review: [], reports: [],
});

export type FrameScheduler = (flush: () => void) => void;

const defaultScheduler: FrameScheduler = (flush) => {
  if (typeof globalThis.requestAnimationFrame === "function") globalThis.requestAnimationFrame(flush);
  else setTimeout(flush, 16);
};

export class Store {
  private latest: State;
  private committed: State;
  private scheduled = false;
  private readonly listeners = new Set<() => void>();
  private readonly schedule: FrameScheduler;

  constructor(initial: State = INITIAL_STATE, schedule: FrameScheduler = defaultScheduler) {
    this.latest = initial;
    this.committed = initial;
    this.schedule = schedule;
  }

  /** The newest state, for the code that writes it. */
  get(): State {
    return this.latest;
  }

  /** The state readers render: it changes at most once per animation frame. */
  getSnapshot = (): State => this.committed;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  update(recipe: (state: State) => State): void {
    const next = recipe(this.latest);
    invariant(next !== null && typeof next === "object", "a store recipe must return the next state");
    if (next === this.latest) return;
    this.latest = next;
    if (this.scheduled) return;
    this.scheduled = true;
    this.schedule(() => this.flush());
  }

  /** Commits the pending state and notifies readers once. */
  flush(): void {
    this.scheduled = false;
    if (this.committed === this.latest) return;
    this.committed = this.latest;
    for (const listener of [...this.listeners]) listener();
  }

  updateAether(conn: ConnId, recipe: (s: AetherConnState) => AetherConnState): void {
    this.update((state) => {
      const before = state.aether[conn] ?? EMPTY_AETHER;
      const after = recipe(before);
      return after === before ? state : { ...state, aether: { ...state.aether, [conn]: after } };
    });
  }

  updateArchon(conn: ConnId, recipe: (s: ArchonConnState) => ArchonConnState): void {
    this.update((state) => {
      const before = state.archon[conn] ?? EMPTY_ARCHON;
      const after = recipe(before);
      return after === before ? state : { ...state, archon: { ...state.archon, [conn]: after } };
    });
  }

  private nextReport = 1;

  report(message: string, conn?: ConnId, code?: string): void {
    const entry: Report = { id: this.nextReport++, message, ...(conn === undefined ? {} : { conn }), ...(code === undefined ? {} : { code }) };
    this.update((state) => ({ ...state, reports: [...state.reports, entry].slice(-50) }));
  }
}

/**
 * Drops every cached entry of a connection whose daemon identity changed
 * (§5.1): cursors, transcripts and outbox. Unconfirmed submissions go to review.
 */
export function dropConnection(state: State, conn: ConnId): State {
  const outbox = [...(state.aether[conn]?.outbox ?? []), ...(state.archon[conn]?.outbox ?? [])];
  const aetherRest = omit(state.aether, conn);
  const archonRest = omit(state.archon, conn);
  const review = [...state.review, ...outbox.map((s) => ({ ...s, state: "unknown" as const }))];
  return { ...state, aether: aetherRest, archon: archonRest, review };
}
