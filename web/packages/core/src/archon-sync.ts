// Keeps one archond connection's cache current (CONTRACTS.md §5.3): change and
// feed cursors never mixed, per-snapshot `as_of`, and the protocol 23 fallback.

import { Debouncer, KeyedSerial } from "./debounce.ts";
import { codeOf } from "./errors.ts";
import { array, errorText, invariant, object, omit, seq, string, type Json } from "./guards.ts";
import { HISTORY_LIMIT, historyValue, initiativeValue, searchCommand, SEARCHES_KEPT, searchKey, searchValue, withStamped, type ItemSearch } from "./archon-reads.ts";
import { LINK_LOST, replyData, replyObject, subscriptionId } from "./replies.ts";
import type { SyncContext } from "./aether-sync.ts";
import { EMPTY_ARCHON, replaces, type ArchonConnState, type ArchonGraph, type ArchonItem, type ArchonPlan } from "./store.ts";

export const FALLBACK_REFETCH_MS = 350;
const ENTITY_REFETCH_MS = 250;
/** Summaries are one list read, so a burst of changes across items rereads it once. */
const SUMMARY_REFETCH_MS = 500;
/** A plan read changes keep overtaking is asked this many times before it is installed anyway. */
const PLAN_READ_ATTEMPTS = 3;
const FEED_KEEP = 200;
/** The most nodes `task_graph` serves (CONTRACTS.md §1.3); the default is 500. */
export const GRAPH_LIMIT = 2000;

/** A queue state is a bare kind or an object tagged by `state`. */
function stateKind(value: unknown): string {
  return typeof value === "string" ? value : string(object(value, "item state").state, "item state kind");
}

export function itemFromSummary(summary: Json, asOf: number | null): ArchonItem {
  const stage = typeof summary.stage === "string" ? summary.stage : undefined;
  const waiting = summary.waiting_reason;
  const waitingReason = typeof waiting === "string" ? waiting : waiting && typeof waiting === "object" ? String((waiting as Json).code ?? "") : undefined;
  return { summary, state: stateKind(summary.state), ...(stage ? { stage } : {}), ...(waitingReason ? { waitingReason } : {}), asOf };
}

/**
 * Installs a summaries read stamped `asOf`: each row replaces its whole item unless the held item is newer,
 * and an item the read lacks is gone unless a change newer than the read put it there.
 */
export function installSummaries(held: Readonly<Record<string, ArchonItem>>, rows: readonly Json[], asOf: number | null): Record<string, ArchonItem> {
  const next: Record<string, ArchonItem> = {};
  for (const [id, item] of Object.entries(held)) if (!replaces(asOf, item.asOf)) next[id] = item;
  for (const row of rows) {
    const id = string(row.id, "item summary id");
    invariant(!(id in next) || next[id] === held[id], `item ${id} appears once in a summaries read`);
    if (!(id in next)) next[id] = itemFromSummary(row, asOf);
  }
  return next;
}

/** A Change's subject fields sit beside `seq` and `at`, tagged by `subject` (CONTRACTS.md §1.3). */
function subjectOf(change: Json): Json {
  string(change.subject, "change.subject");
  return change;
}

/** Applies one Change to the snapshots older than it (§5.3.2); replaying a change is harmless. */
export function patchSnapshots(state: ArchonConnState, value: Json): ArchonConnState {
  const at = seq(value.seq, "change.seq");
  const subject = subjectOf(value);
  const graph = state.graph && at > state.graph.asOf ? state.graph : null;
  if (subject.subject === "initiative") return graph ? { ...state, graph: withInitiative(graph, subject) } : state;
  const item = typeof subject.item === "string" ? subject.item : undefined;
  if (!item) return state;
  let next = state;
  if (subject.subject === "item") {
    const live = { state: string(subject.state, "change.state"), stage: subject.stage, waitingReason: subject.waiting_reason };
    const current = next.items[item];
    if (current && (current.asOf === null || at > current.asOf)) {
      // §5.3.3 moves the live position in place; the summary stays whole until its reread replaces it.
      const patched: ArchonItem = {
        summary: current.summary, state: live.state, asOf: at,
        ...(typeof live.stage === "string" ? { stage: live.stage } : {}), ...(typeof live.waitingReason === "string" ? { waitingReason: live.waitingReason } : {}),
      };
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
  const subject = subjectOf(value);
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

/** Drops what an off-screen item's change made old; its screen reads them again when shown. */
function withoutItem(state: ArchonConnState, item: string): ArchonConnState {
  const runs = Object.entries(state.plans).filter(([, plan]) => plan.item === item).map(([run]) => run);
  if (!(item in state.details) && !(item in state.links) && !(item in state.histories) && runs.length === 0) return state;
  const plans = runs.reduce<Readonly<Record<string, ArchonPlan>>>((rest, run) => omit(rest, run), state.plans);
  return { ...state, details: omit(state.details, item), links: omit(state.links, item), histories: omit(state.histories, item), plans };
}

/** The serial key of summary reads; item ids never start with a colon. */
const SUMMARIES = ":summaries";

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
  private readonly summaries = new Debouncer(SUMMARY_REFETCH_MS);
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
      await this.serial.run(SUMMARIES, () => this.readSummaries());
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
    this.set((s) => ({ ...s, changeCursor: null, details: {}, links: {}, plans: {}, histories: {}, staleItems: [...new Set([...s.staleItems, ...onScreen])] }));
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
    this.summaries.cancelAll();
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
    const summary = item !== undefined && !state.staleSummaries.includes(item);
    const forgotten = item !== undefined && !visible ? withoutItem(state, item) : state;
    // Marked with the cursor advance, so a reconnect cannot skip what this change asked for.
    this.set(() => ({
      ...forgotten, ...(unknown ? { staleSnapshots: true } : {}), ...(detail ? { staleItems: [...state.staleItems, item] } : {}),
      ...(summary ? { staleSummaries: [...state.staleSummaries, item] } : {}),
    }));
    this.collecting?.push(change);
    this.scheduleStale();
  }

  private scheduleStale(): void {
    const { staleSnapshots, staleItems, staleSummaries } = this.state;
    if (staleSnapshots) this.structure.schedule("snapshots", () => void this.rereadSnapshots());
    if (staleSummaries.length > 0) this.summaries.schedule("summaries", () => void this.serial.run(SUMMARIES, () => this.refetchSummaries()));
    for (const item of staleItems) this.refetch.schedule(item, () => void this.serial.run(item, () => this.refetchItem(item)));
    const stale = staleSnapshots || staleItems.length > 0 || staleSummaries.length > 0;
    invariant(this.changeSub !== null || !stale, "stale entities come from a change subscription");
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
    // Serialized, so an older reply cannot land after a newer one.
    this.fallback.schedule("summaries", () => void this.serial.run(SUMMARIES, () => this.readSummaries()).catch((error: unknown) => {
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
    const graph = replyObject(await this.ctx.request({ cmd: "task_graph", filter: { include_terminal_since: "1970-01-01T00:00:00Z" }, limit: GRAPH_LIMIT }), "task_graph");
    const nodes = Object.fromEntries(array(graph.nodes, "task_graph.nodes").map((node) => [string(object(node, "node").id, "node.id"), node as Json]));
    const snapshot: ArchonGraph = {
      nodes, edges: array(graph.edges, "task_graph.edges") as Json[], initiatives: array(graph.initiatives ?? [], "initiatives") as Json[],
      asOf: seq(graph.as_of, "task_graph.as_of"), truncated: graph.truncated === true,
    };
    invariant(Object.keys(nodes).length <= GRAPH_LIMIT, `task_graph sent more than ${GRAPH_LIMIT} nodes`);
    // A read older than the graph held, e.g. one a concurrent reread overtook, never replaces it.
    this.set((s) => (replaces(snapshot.asOf, s.graph?.asOf ?? null) ? { ...s, graph: snapshot } : s));
  }

  /** Protocol 24 asks `with_as_of` and gets `item_summaries_at`; protocol 23 answers the bare list. */
  private async readSummaries(): Promise<void> {
    const reply = await this.ctx.request(this.usesChanges ? { cmd: "list_item_summaries", with_as_of: true } : { cmd: "list_item_summaries" });
    const data = replyData(reply, reply.reply === "item_summaries_at" ? "item_summaries_at" : "item_summaries");
    const wrapped = Array.isArray(data) ? null : object(data, "item_summaries");
    const rows = array(wrapped ? wrapped.summaries ?? wrapped.items : data, "item summaries").map((row) => object(row, "item summary"));
    const asOf = wrapped ? seq(wrapped.as_of, "item_summaries.as_of") : null;
    this.set((s) => ({ ...s, items: installSummaries(s.items, rows, asOf), itemsAsOf: asOf }));
  }

  /** Rereads the summaries changes named; a change during the read marks its item again. */
  private async refetchSummaries(): Promise<void> {
    const marked = new Set(this.state.staleSummaries);
    if (marked.size === 0) return;
    this.set((s) => ({ ...s, staleSummaries: s.staleSummaries.filter((id) => !marked.has(id)) }));
    try {
      await this.readSummaries();
    } catch (error) {
      if (!LINK_LOST.has(codeOf(error) ?? "")) return this.fail("the queue", error);
      this.set((s) => ({ ...s, staleSummaries: [...new Set([...s.staleSummaries, ...marked])] }));
      this.retryAfterTimeout("the queue", error);
    }
  }

  /** Rereads an item's detail, links and plans into the store now: protocol 23 polling, or after an action. */
  refresh(item: string): Promise<void> {
    invariant(item.length > 0, "a refresh names an item");
    return this.serial.run(item, () => this.readItem(item));
  }

  /**
   * Reads a run's plan into the store, or rejects so the screen shows why; the item's rereads keep it current.
   * A read a change overtook is asked again; the last one is stamped when asked and the item's reread replaces it.
   */
  readPlan(item: string, run: string): Promise<void> {
    invariant(this.ctx.features.has("run_plan"), "run_plan is read only where archond serves it");
    return this.serial.run(item, async () => {
      if (this.state.plans[run] !== undefined) return;
      for (let attempt = 1; attempt <= PLAN_READ_ATTEMPTS; attempt += 1) {
        const version = this.changed.get(item) ?? 0;
        const asked = this.state.changeCursor;
        const plan = await this.fetchPlan(item, run);
        if (!this.overtaken(item, version)) return this.installPlans([[run, plan]]);
        if (attempt === PLAN_READ_ATTEMPTS) return this.installPlans([[run, { ...plan, asOf: asked }]]);
      }
    });
  }

  private installPlans(plans: readonly (readonly [string, ArchonPlan])[]): void {
    invariant(plans.every(([run, plan]) => run.length > 0 && plan.item.length > 0), "an installed plan names its run and item");
    if (plans.length > 0) this.set((s) => ({ ...s, plans: { ...s.plans, ...Object.fromEntries(plans) } }));
  }

  private async fetchPlan(item: string, run: string): Promise<ArchonPlan> {
    const value = replyObject(await this.ctx.request({ cmd: "run_plan", run }), "run_plan");
    invariant(value.run === undefined || value.run === run, `the plan of ${run} describes ${String(value.run)}`);
    return { value, asOf: this.state.changeCursor, item };
  }

  /** Whether a change to `item` arrived since `version`; it re-marked the item if visible, or dropped its rereads. */
  private overtaken(item: string, version: number): boolean {
    if ((this.changed.get(item) ?? 0) === version) return false;
    invariant(!this.onScreen.has(item) || !this.usesChanges || this.state.staleItems.includes(item), "an overtaken visible item is refetched again");
    return true;
  }

  /**
   * Every reply is stamped with the change cursor at install. No change to the item arrived since it was
   * asked for, so it reflects at least every change of the item seen; an overtaken reply is not installed.
   */
  private async readItem(item: string): Promise<void> {
    const version = this.changed.get(item) ?? 0;
    const detail = replyObject(await this.ctx.request({ cmd: "get_item", item }), "item_detail");
    const links = this.ctx.features.has("github_links") ? replyObject(await this.ctx.request({ cmd: "item_links", item }), "item_links") : null;
    if (this.overtaken(item, version)) return;
    const asOf = this.state.changeCursor;
    this.set((s) => ({
      ...s, details: { ...s.details, [item]: { value: detail, asOf } },
      links: links === null ? s.links : { ...s.links, [item]: { value: links, asOf } },
    }));
    await this.readItemPlans(item, version);
    if (this.ctx.features.has("item_history") && item in this.state.histories) await this.readItemHistory(item);
  }

  /** Reads an item's transitions into the store; while the item is on screen its rereads keep them current. */
  async readHistory(item: string): Promise<void> {
    invariant(this.ctx.features.has("item_history"), "item_history is read only where archond serves it");
    return this.serial.run(item, () => this.readItemHistory(item));
  }

  /** As readPlan: a read a change overtook is asked again, and the last one is stamped when it was asked. */
  private async readItemHistory(item: string): Promise<void> {
    for (let attempt = 1; attempt <= PLAN_READ_ATTEMPTS; attempt += 1) {
      const version = this.changed.get(item) ?? 0;
      const asked = this.state.changeCursor;
      const value = historyValue(await this.ctx.request({ cmd: "item_history", item, limit: HISTORY_LIMIT }), item);
      const fresh = !this.overtaken(item, version);
      if (fresh || attempt === PLAN_READ_ATTEMPTS) {
        const stamped = { value, asOf: fresh ? this.state.changeCursor : asked };
        return this.set((s) => ({ ...s, histories: withStamped(s.histories, item, stamped) }));
      }
    }
  }

  /** Reads an initiative's detail into the store, stamped with the change cursor it was asked at. */
  async readInitiative(id: string): Promise<void> {
    invariant(id.length > 0, "an initiative read names its initiative");
    return this.serial.run(`:initiative:${id}`, async () => {
      const asOf = this.state.changeCursor;
      const value = initiativeValue(await this.ctx.request({ cmd: "get_initiative", initiative: id }), id);
      this.set((s) => ({ ...s, initiatives: withStamped(s.initiatives, id, { value, asOf }) }));
    });
  }

  /** Runs a filtered `list_items` into the store under `searchKey(query)`; the store keeps the newest few. */
  async search(query: ItemSearch): Promise<void> {
    invariant(this.ctx.features.has("item_filters"), "list_items filters are sent only where archond serves them");
    const key = searchKey(query);
    return this.serial.run(`:search:${key}`, async () => {
      const asOf = this.state.changeCursor;
      const value = searchValue(await this.ctx.request(searchCommand(query)));
      this.set((s) => ({ ...s, searches: withStamped(s.searches, key, { value, asOf }, SEARCHES_KEPT) }));
    });
  }

  /** Rereads the plans held for `item`; one the daemon refuses is reported and dropped from the store. */
  private async readItemPlans(item: string, version: number): Promise<void> {
    const runs = Object.entries(this.state.plans).filter(([, plan]) => plan.item === item).map(([run]) => run);
    const results = await Promise.allSettled(runs.map((run) => this.fetchPlan(item, run)));
    invariant(results.length === runs.length, "every plan read is answered");
    const lost = results.find((result) => result.status === "rejected" && LINK_LOST.has(codeOf(result.reason) ?? ""));
    if (lost?.status === "rejected") throw lost.reason;
    if (this.overtaken(item, version)) return;
    const installed: (readonly [string, ArchonPlan])[] = [];
    const refused: string[] = [];
    results.forEach((result, index) => {
      const run = runs[index] as string;
      if (result.status === "fulfilled") installed.push([run, result.value]);
      else {
        refused.push(run);
        this.fail(`the plan of ${run}`, result.reason);
      }
    });
    // A refused plan is not kept as if current; a screen showing it reads it again and shows why it fails.
    if (refused.length > 0) this.set((s) => ({ ...s, plans: refused.reduce<Readonly<Record<string, ArchonPlan>>>((rest, run) => omit(rest, run), s.plans) }));
    this.installPlans(installed);
  }

  /** Clears the marker first; a lost link puts it back for the next ready, a daemon answer does not. */
  private async refetchItem(item: string): Promise<void> {
    this.set((s) => ({ ...s, staleItems: s.staleItems.filter((id) => id !== item) }));
    try {
      await this.readItem(item);
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
