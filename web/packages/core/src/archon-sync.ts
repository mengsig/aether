// Keeps one archond connection's cache current (CONTRACTS.md §5.3): change and
// feed cursors never mixed, per-snapshot `as_of`, and the protocol 23 fallback.

import { Debouncer } from "./debounce.ts";
import { codeOf } from "./errors.ts";
import { array, errorText, invariant, object, seq, string, type Json } from "./guards.ts";
import { replyData, replyObject, subscriptionId } from "./replies.ts";
import type { SyncContext } from "./aether-sync.ts";
import { EMPTY_ARCHON, type ArchonConnState, type ArchonGraph, type ArchonItem } from "./store.ts";

export const FALLBACK_REFETCH_MS = 350;
const ENTITY_REFETCH_MS = 250;
const FEED_KEEP = 200;

/** A queue state is a bare kind or an object tagged by `state`. */
function stateKind(value: unknown): string {
  return typeof value === "string" ? value : string(object(value, "item state").state, "item state kind");
}

export function itemFromSummary(summary: Json): ArchonItem {
  const stage = typeof summary.stage === "string" ? summary.stage : undefined;
  const waiting = summary.waiting_reason;
  const waitingReason = typeof waiting === "string" ? waiting : waiting && typeof waiting === "object" ? String((waiting as Json).code ?? "") : undefined;
  return { summary, state: stateKind(summary.state), ...(stage ? { stage } : {}), ...(waitingReason ? { waitingReason } : {}) };
}

/** Applies one Change to the snapshots older than it (§5.3.2); replaying a change is harmless. */
export function patchSnapshots(state: ArchonConnState, value: Json): ArchonConnState {
  const at = seq(value.seq, "change.seq");
  const subject = object(value.subject, "change.subject");
  const item = typeof subject.item === "string" ? subject.item : undefined;
  if (!item) return state;
  let next = state;
  if (subject.subject === "item") {
    const live = { state: string(subject.state, "change.state"), stage: subject.stage, waitingReason: subject.waiting_reason };
    const current = next.items[item];
    if (current && (next.itemsAsOf === null || at > next.itemsAsOf)) {
      const patched: ArchonItem = { summary: current.summary, state: live.state, ...(typeof live.stage === "string" ? { stage: live.stage } : {}), ...(typeof live.waitingReason === "string" ? { waitingReason: live.waitingReason } : {}) };
      next = { ...next, items: { ...next.items, [item]: patched } };
    }
    const node = next.graph?.nodes[item];
    if (next.graph && node && at > next.graph.asOf) next = { ...next, graph: { ...next.graph, nodes: { ...next.graph.nodes, [item]: { ...node, state: live.state } } } };
  }
  if (subject.subject === "dependency" && next.graph && at > next.graph.asOf) next = { ...next, graph: withDependency(next.graph, subject) };
  return next;
}

/**
 * Applies one Change past the cursor (§5.3.2-3). `unknown` means it names an
 * item missing from a snapshot, which then needs rereading.
 */
export function applyChange(state: ArchonConnState, value: Json): { state: ArchonConnState; item?: string; unknown: boolean } {
  const at = seq(value.seq, "change.seq");
  if (state.changeCursor !== null && at <= state.changeCursor) return { state, unknown: false };
  const subject = object(value.subject, "change.subject");
  const item = typeof subject.item === "string" ? subject.item : undefined;
  const next = patchSnapshots({ ...state, changeCursor: at }, value);
  invariant(next.changeCursor === at, "an applied change advances the change cursor");
  const named = item === undefined ? [] : subject.subject === "dependency" ? [item, string(subject.depends_on, "dependency.depends_on")] : [item];
  const unknown = named.some((id) => !(id in next.items) || (next.graph !== null && !next.graph.truncated && !(id in next.graph.nodes)));
  return { state: next, ...(item ? { item } : {}), unknown };
}

function withDependency(graph: ArchonGraph, subject: Json): ArchonGraph {
  const from = string(subject.depends_on, "dependency.depends_on");
  const to = string(subject.item, "dependency.item");
  const others = graph.edges.filter((edge) => !(edge.from === from && edge.to === to && edge.kind === "depends_on"));
  const edges = subject.added === true ? [...others, { from, to, kind: "depends_on", satisfied: false }] : others;
  return { ...graph, edges };
}

export class ArchonSync {
  private readonly ctx: SyncContext;
  private changeSub: number | null = null;
  private feedSub: number | null = null;
  private buffer: Json[] | null = null;
  private readonly refetch = new Debouncer(ENTITY_REFETCH_MS);
  private readonly fallback = new Debouncer(FALLBACK_REFETCH_MS);
  private readonly structure = new Debouncer(ENTITY_REFETCH_MS);
  /** Every change applied while snapshots are reread, replayed onto them once installed. */
  private collecting: Json[] | null = null;
  private rereadAgain = false;
  private onScreen: ReadonlySet<string> = new Set();

  constructor(ctx: SyncContext) {
    this.ctx = ctx;
  }

  private get state(): ArchonConnState {
    return this.ctx.store.get().archon[this.ctx.conn] ?? EMPTY_ARCHON;
  }

  private set(recipe: (s: ArchonConnState) => ArchonConnState): void {
    this.ctx.store.updateArchon(this.ctx.conn, recipe);
  }

  get usesChanges(): boolean {
    return this.ctx.features.has("changes");
  }

  async start(): Promise<void> {
    if (!this.usesChanges) {
      await this.subscribeFeed();
      await this.readSummaries();
      return;
    }
    this.buffer = [];
    const cursor = this.state.changeCursor;
    try {
      this.changeSub = subscriptionId(await this.ctx.request({ cmd: "subscribe_changes", after: cursor }));
    } catch (error) {
      if (cursor === null || codeOf(error) !== "history_expired") throw error;
      await this.expired();
      return this.finishStart();
    }
    if (cursor === null) await this.readSnapshots();
    await this.finishStart();
  }

  private async finishStart(): Promise<void> {
    const buffered = this.buffer ?? [];
    this.buffer = null;
    for (const frame of buffered) this.onChange(frame);
    await this.subscribeFeed();
  }

  /** §5.3.4: a fresh task_graph, then changes after its as_of. */
  private async expired(): Promise<void> {
    this.set((s) => ({ ...s, changeCursor: null }));
    await this.readSnapshots();
    const asOf = this.state.changeCursor;
    this.changeSub = subscriptionId(await this.ctx.request({ cmd: "subscribe_changes", after: asOf }));
  }

  stop(): void {
    this.changeSub = null;
    this.feedSub = null;
    this.buffer = null;
    this.refetch.cancelAll();
    this.fallback.cancelAll();
    this.structure.cancelAll();
    this.collecting = null;
    this.rereadAgain = false;
  }

  /** Entities on screen are refetched on change; others are updated in place only. */
  show(items: readonly string[]): void {
    this.onScreen = new Set(items);
  }

  onChange(frame: Json): void {
    if (this.buffer) {
      this.buffer.push(frame);
      return;
    }
    if (frame.sub !== this.changeSub) return;
    const change = object(frame.change, "change frame");
    const before = this.state;
    const { state, item, unknown } = applyChange(before, change);
    if (state === before) return;
    this.set(() => state);
    this.collecting?.push(change);
    if (unknown) this.structure.schedule("snapshots", () => void this.rereadSnapshots().catch((error: unknown) => this.fail("the queue", error)));
    if (item && this.onScreen.has(item)) this.refetch.schedule(item, () => void this.refetchItem(item));
  }

  onFeed(frame: Json): void {
    if (frame.sub !== this.feedSub) return;
    const item = object(frame.item, "feed item");
    const at = seq(item.seq, "feed item seq");
    const feedSeq = this.state.feedSeq;
    if (feedSeq !== null && at <= feedSeq) return;
    this.set((s) => ({ ...s, feedSeq: at, feed: [...s.feed, item].slice(-FEED_KEEP) }));
    // §5.3.5: without `changes`, summaries are refetched 350 ms after any feed item.
    if (!this.usesChanges) this.fallback.schedule("summaries", () => void this.readSummaries().catch((error: unknown) => this.fail("summaries", error)));
  }

  private async subscribeFeed(): Promise<void> {
    let after = this.state.feedSeq;
    if (after === null) {
      const history = array(replyData(await this.ctx.request({ cmd: "feed_history", before: null, limit: FEED_KEEP, verbose: false }), "feed"), "feed history");
      const items = history.map((value) => object(value, "feed item"));
      after = items.reduce((max, item) => Math.max(max, seq(item.seq, "feed item seq")), 0);
      const last = after;
      this.set((s) => ({ ...s, feed: items.slice(-FEED_KEEP), feedSeq: last }));
    }
    this.feedSub = subscriptionId(await this.ctx.request({ cmd: "subscribe_feed", after, verbose: true }));
  }

  private async readSnapshots(): Promise<void> {
    await this.readGraph();
    await this.readSummaries();
    // Changes at or below every snapshot's as_of are already in all of them.
    const asOfs = [this.state.graph?.asOf, this.state.itemsAsOf].filter((value): value is number => typeof value === "number");
    if (asOfs.length > 0) this.set((s) => ({ ...s, changeCursor: Math.min(...asOfs) }));
  }

  /** A change named an entity no snapshot holds: reread both, keeping the cursor. */
  private async rereadSnapshots(): Promise<void> {
    if (this.collecting) {
      this.rereadAgain = true;
      return;
    }
    invariant(this.changeSub !== null, "snapshots are reread only while changes are subscribed");
    const collected: Json[] = [];
    this.collecting = collected;
    try {
      await this.readGraph();
      await this.readSummaries();
      // Changes that arrived while the snapshots were read may be newer than their as_of.
      this.set((s) => collected.reduce(patchSnapshots, s));
    } finally {
      this.collecting = null;
    }
    if (this.rereadAgain) {
      this.rereadAgain = false;
      await this.rereadSnapshots();
    }
  }

  private async readGraph(): Promise<void> {
    if (!this.ctx.features.has("task_graph")) return;
    const graph = replyObject(await this.ctx.request({ cmd: "task_graph", filter: {} }), "task_graph");
    const nodes = Object.fromEntries(array(graph.nodes, "task_graph.nodes").map((node) => [string(object(node, "node").id, "node.id"), node as Json]));
    const snapshot: ArchonGraph = {
      nodes, edges: array(graph.edges, "task_graph.edges") as Json[], initiatives: array(graph.initiatives ?? [], "initiatives") as Json[],
      asOf: seq(graph.as_of, "task_graph.as_of"), truncated: graph.truncated === true,
    };
    this.set((s) => ({ ...s, graph: snapshot }));
  }

  /** Protocol 24 wraps the rows with their `as_of`; protocol 23 answers the bare list. */
  private async readSummaries(): Promise<void> {
    const data = replyData(await this.ctx.request({ cmd: "list_item_summaries" }), "item_summaries");
    const wrapped = Array.isArray(data) ? null : object(data, "item_summaries");
    const rows = array(wrapped ? wrapped.summaries ?? wrapped.items : data, "item summaries");
    const asOf = wrapped ? seq(wrapped.as_of, "item_summaries.as_of") : null;
    const items = Object.fromEntries(rows.map((row) => {
      const summary = object(row, "item summary");
      return [string(summary.id, "item summary id"), itemFromSummary(summary)];
    }));
    this.set((s) => ({ ...s, items, itemsAsOf: asOf }));
  }

  private async refetchItem(item: string): Promise<void> {
    try {
      const detail = replyObject(await this.ctx.request({ cmd: "get_item", item }), "item");
      this.set((s) => ({ ...s, details: { ...s.details, [item]: detail } }));
    } catch (error) {
      this.fail(`item ${item}`, error);
    }
  }

  private fail(what: string, error: unknown): void {
    this.ctx.store.report(`Could not refresh ${what}: ${errorText(error)}`, this.ctx.conn, codeOf(error));
  }
}
