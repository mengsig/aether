// Keeps one archond connection's cache current (CONTRACTS.md §5.3): change and
// feed cursors never mixed, per-snapshot `as_of`, and the protocol 23 fallback.

import { Debouncer, KeyedSerial } from "./debounce.ts";
import { codeOf } from "./errors.ts";
import { array, errorText, invariant, object, omit, seq, string, type Json } from "./guards.ts";
import { LINK_LOST, replyData, replyObject, subscriptionId } from "./replies.ts";
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
  const graph = state.graph && at > state.graph.asOf ? state.graph : null;
  if (subject.subject === "initiative") return graph ? { ...state, graph: withInitiative(graph, subject) } : state;
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
    const node = graph?.nodes[item];
    if (graph && node) next = { ...next, graph: { ...graph, nodes: { ...graph.nodes, [item]: withLive(node, subject) } } };
  }
  if (subject.subject === "dependency" && graph) next = { ...next, graph: withDependency(graph, subject) };
  return next;
}

/** The ids a change names that a snapshot should hold; a missing one means the snapshots need rereading. */
function namedMissing(state: ArchonConnState, subject: Json): boolean {
  const graph = state.graph;
  if (subject.subject === "initiative") {
    const id = string(subject.initiative, "change.initiative");
    return graph !== null && !graph.initiatives.some((initiative) => initiative.id === id);
  }
  if (typeof subject.item !== "string") return false;
  const named = subject.subject === "dependency" ? [subject.item, string(subject.depends_on, "dependency.depends_on")] : [subject.item];
  return named.some((id) => !(id in state.items) || (graph !== null && !graph.truncated && !(id in graph.nodes)));
}

/** `satisfied` is derived by archond: a moved prerequisite or a new edge leaves it unknown here. */
function edgesUnknown(before: ArchonConnState, at: number, subject: Json): boolean {
  const graph = before.graph;
  if (!graph || at <= graph.asOf) return false;
  if (subject.subject === "dependency") return subject.added === true;
  if (subject.subject !== "item" || typeof subject.item !== "string") return false;
  const item = subject.item;
  if (graph.nodes[item]?.state === subject.state) return false;
  return graph.edges.some((edge) => edge.from === item);
}

/**
 * Applies one Change past the cursor (§5.3.2-3). `unknown` means a snapshot
 * lacks what it names or cannot derive its effect, and needs rereading.
 */
export function applyChange(state: ArchonConnState, value: Json): { state: ArchonConnState; item?: string; unknown: boolean } {
  const at = seq(value.seq, "change.seq");
  if (state.changeCursor !== null && at <= state.changeCursor) return { state, unknown: false };
  const subject = object(value.subject, "change.subject");
  const item = typeof subject.item === "string" ? subject.item : undefined;
  const next = patchSnapshots({ ...state, changeCursor: at }, value);
  invariant(next.changeCursor === at, "an applied change advances the change cursor");
  const unknown = namedMissing(next, subject) || edgesUnknown(state, at, subject);
  return { state: next, ...(item ? { item } : {}), unknown };
}

/** An item change carries the node's whole live position; `state_detail` describes the old state only. */
function withLive(node: Json, subject: Json): Json {
  const state = string(subject.state, "change.state");
  invariant(subject.subject === "item", "only item changes move a node");
  const rest = Object.fromEntries(Object.entries(node).filter(([key]) => !["stage", "waiting_reason", "state_detail"].includes(key)));
  const detail = node.state_detail;
  return {
    ...rest, state,
    ...(typeof subject.stage === "string" ? { stage: subject.stage } : {}),
    ...(typeof subject.waiting_reason === "string" ? { waiting_reason: subject.waiting_reason } : {}),
    ...(detail !== undefined && node.state === state ? { state_detail: detail } : {}),
  };
}

function withInitiative(graph: ArchonGraph, subject: Json): ArchonGraph {
  const id = string(subject.initiative, "change.initiative");
  const state = string(subject.state, "change.state");
  const initiatives = graph.initiatives.map((initiative) => (initiative.id === id ? { ...initiative, state } : initiative));
  return { ...graph, initiatives };
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
  /** Feed frames that arrive before `subscribe_feed`'s answer is read. */
  private feedBuffer: Json[] | null = null;
  private readonly refetch = new Debouncer(ENTITY_REFETCH_MS);
  private readonly fallback = new Debouncer(FALLBACK_REFETCH_MS);
  private readonly structure = new Debouncer(ENTITY_REFETCH_MS);
  private readonly serial = new KeyedSerial();
  /** Every change applied while snapshots are reread, replayed onto them once installed. */
  private collecting: Json[] | null = null;
  private rereadAgain = false;
  private onScreen: ReadonlySet<string> = new Set();
  /** Changes applied per item, so a detail read that one overtook is not installed. */
  private readonly changed = new Map<string, number>();

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
    this.scheduleStale();
  }

  /** §5.3.4: a fresh task_graph, then changes after its as_of; skipped changes may have moved any detail. */
  private async expired(): Promise<void> {
    const onScreen = [...this.onScreen];
    this.set((s) => ({ ...s, changeCursor: null, details: {}, staleItems: [...new Set([...s.staleItems, ...onScreen])] }));
    await this.readSnapshots();
    const asOf = this.state.changeCursor;
    this.changeSub = subscriptionId(await this.ctx.request({ cmd: "subscribe_changes", after: asOf }));
  }

  stop(): void {
    this.changeSub = null;
    this.feedSub = null;
    this.buffer = null;
    this.feedBuffer = null;
    this.refetch.cancelAll();
    this.fallback.cancelAll();
    this.structure.cancelAll();
    this.collecting = null;
    this.rereadAgain = false;
  }

  /** Items on screen are refetched on change; off-screen details are dropped then and fetched on return. */
  show(items: readonly string[]): void {
    this.onScreen = new Set(items);
    if (!this.usesChanges) return;
    const missing = items.filter((item) => !(item in this.state.details) && !this.state.staleItems.includes(item));
    if (missing.length === 0) return;
    this.set((s) => ({ ...s, staleItems: [...s.staleItems, ...missing] }));
    // Before the change subscription exists, finishStart schedules the markers.
    if (this.changeSub !== null && this.buffer === null) this.scheduleStale();
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
    if (item !== undefined) this.changed.set(item, (this.changed.get(item) ?? 0) + 1);
    const visible = item !== undefined && this.onScreen.has(item);
    const detail = visible && !state.staleItems.includes(item);
    const details = item !== undefined && !visible && item in state.details ? omit(state.details, item) : state.details;
    // Marked with the cursor advance, so a reconnect cannot skip what this change asked for.
    this.set(() => ({ ...state, details, ...(unknown ? { staleSnapshots: true } : {}), ...(detail ? { staleItems: [...state.staleItems, item] } : {}) }));
    this.collecting?.push(change);
    this.scheduleStale();
  }

  private scheduleStale(): void {
    const { staleSnapshots, staleItems } = this.state;
    if (staleSnapshots) this.structure.schedule("snapshots", () => void this.rereadSnapshots());
    for (const item of staleItems) this.refetch.schedule(item, () => void this.serial.run(item, () => this.refetchItem(item)));
    invariant(this.changeSub !== null || (!staleSnapshots && staleItems.length === 0), "stale entities come from a change subscription");
  }

  onFeed(frame: Json): void {
    if (this.feedBuffer) {
      this.feedBuffer.push(frame);
      return;
    }
    if (frame.sub !== this.feedSub) return;
    const item = object(frame.item, "feed item");
    const at = seq(item.seq, "feed item seq");
    const feedSeq = this.state.feedSeq;
    if (feedSeq !== null && at <= feedSeq) return;
    this.set((s) => ({ ...s, feedSeq: at, feed: [...s.feed, item].slice(-FEED_KEEP) }));
    // §5.3.5: without `changes`, summaries are refetched 350 ms after any feed item.
    if (!this.usesChanges) this.refreshSummaries();
  }

  /** A timeout on a live link reschedules; after a disconnect the next start reads the summaries. */
  private refreshSummaries(): void {
    invariant(!this.usesChanges, "only protocol 23 refetches summaries on feed items");
    this.fallback.schedule("summaries", () => void this.readSummaries().catch((error: unknown) => {
      this.fail("summaries", error);
      if (codeOf(error) === "timeout" && this.feedSub !== null) this.refreshSummaries();
    }));
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
    // A replay may arrive before the continuation below assigns the subscription id.
    const buffered: Json[] = [];
    this.feedBuffer = buffered;
    try {
      this.feedSub = subscriptionId(await this.ctx.request({ cmd: "subscribe_feed", after, verbose: true }));
    } finally {
      if (this.feedBuffer === buffered) this.feedBuffer = null;
    }
    invariant(this.feedBuffer === null, "feed frames are buffered only while subscribing");
    for (const frame of buffered) this.onFeed(frame);
  }

  private async readSnapshots(): Promise<void> {
    await this.readGraph();
    await this.readSummaries();
    // Changes at or below every snapshot's as_of are already in all of them.
    const asOfs = [this.state.graph?.asOf, this.state.itemsAsOf].filter((value): value is number => typeof value === "number");
    // Fresh snapshots supersede a pending reread; buffered changes re-mark what they need.
    if (asOfs.length > 0) this.set((s) => ({ ...s, changeCursor: Math.min(...asOfs), staleSnapshots: false }));
  }

  /** A change named an entity no snapshot holds: reread both, keeping the cursor. */
  private async rereadSnapshots(): Promise<void> {
    if (this.collecting) {
      this.rereadAgain = true;
      return;
    }
    invariant(this.changeSub !== null, "snapshots are reread only while changes are subscribed");
    this.set((s) => ({ ...s, staleSnapshots: false }));
    const collected: Json[] = [];
    this.collecting = collected;
    try {
      await this.readGraph();
      await this.readSummaries();
    } catch (error) {
      if (!LINK_LOST.has(codeOf(error) ?? "")) this.fail("the queue", error);
      else {
        this.set((s) => ({ ...s, staleSnapshots: true }));
        this.retryAfterTimeout("the queue", error);
      }
    } finally {
      // Whichever snapshots were installed get the changes that arrived while they were read.
      this.set((s) => collected.reduce(patchSnapshots, s));
      if (this.collecting === collected) this.collecting = null;
    }
    if (!this.rereadAgain || this.changeSub === null) return;
    this.rereadAgain = false;
    await this.rereadSnapshots();
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

  /** Clears the marker first; a lost link puts it back for the next ready, a daemon answer does not. */
  private async refetchItem(item: string): Promise<void> {
    this.set((s) => ({ ...s, staleItems: s.staleItems.filter((id) => id !== item) }));
    const version = this.changed.get(item) ?? 0;
    try {
      const detail = replyObject(await this.ctx.request({ cmd: "get_item", item }), "item");
      if ((this.changed.get(item) ?? 0) !== version) {
        // A later change made this reply old: it re-marked the item if visible, or dropped its detail.
        invariant(!this.onScreen.has(item) || this.state.staleItems.includes(item), "an overtaken visible item is refetched again");
        return;
      }
      this.set((s) => ({ ...s, details: { ...s.details, [item]: detail } }));
    } catch (error) {
      if (!LINK_LOST.has(codeOf(error) ?? "")) return this.fail(`item ${item}`, error);
      this.set((s) => (s.staleItems.includes(item) ? s : { ...s, staleItems: [...s.staleItems, item] }));
      this.retryAfterTimeout(`item ${item}`, error);
    }
  }

  /** A lost link refetches on the next ready; a timeout on a live one must be retried now, and seen. */
  private retryAfterTimeout(what: string, error: unknown): void {
    if (codeOf(error) !== "timeout" || this.changeSub === null) return;
    this.fail(what, error);
    this.scheduleStale();
  }

  private fail(what: string, error: unknown): void {
    this.ctx.store.report(`Could not refresh ${what}: ${errorText(error)}`, this.ctx.conn, codeOf(error));
  }
}
