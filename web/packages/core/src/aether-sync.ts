// Keeps one Aether connection's cache current (CONTRACTS.md §5.2): durable
// cursor, snapshots, deltas for on-screen agents and transcript rereads.

import { aether, type ConnId } from "@aether-ui/protocol";
import { applyDelta, applyEvent, discardStreams, endRecovery, mergeEntries } from "./aether-reduce.ts";
import { Debouncer, KeyedSerial } from "./debounce.ts";
import { codeOf } from "./errors.ts";
import { array, errorText, invariant, object, seq, string, type Json } from "./guards.ts";
import { LINK_LOST, RESYNC_CODES, replyData, replyObject, subscriptionId } from "./replies.ts";
import type { Command } from "./rpc.ts";
import { EMPTY_AETHER, type AetherConnState, type Store } from "./store.ts";

export interface SyncContext {
  readonly conn: ConnId;
  readonly store: Store;
  readonly features: ReadonlySet<string>;
  request(command: Command): Promise<Json>;
  /** A setting that feeds Capabilities changed. */
  settingsChanged(): void;
}

/** The client uses at most 4 of the daemon's 16 subscriptions: one durable, three for deltas. */
export const MAX_DELTA_AGENTS = 3;
const REFETCH_MS = 250;
const MAX_PAGES = 100;
/** Bounds one reconciliation of delta subscriptions: each step adds or drops one. */
const MAX_DELTA_STEPS = 16;

export class AetherSync {
  private readonly ctx: SyncContext;
  private durableSub: number | null = null;
  private buffer: Json[] | null = null;
  private readonly deltaSubs = new Map<string, number>();
  private watched: readonly string[] = [];
  private readonly rereading = new Map<string, Promise<void>>();
  private readonly again = new Set<string>();
  private readonly refetch = new Debouncer(REFETCH_MS);
  private readonly serial = new KeyedSerial();
  private reconciling: Promise<void> = Promise.resolve();
  private running = false;

  constructor(ctx: SyncContext) {
    this.ctx = ctx;
  }

  private get state(): AetherConnState {
    return this.ctx.store.get().aether[this.ctx.conn] ?? EMPTY_AETHER;
  }

  /** §5.2.1 and §5.2.5: resume after last_seq, or resync from snapshots when there is none or it expired. */
  async start(): Promise<void> {
    invariant(!this.running, "an Aether sync starts once per ready connection");
    this.running = true;
    this.buffer = [];
    const after = this.state.lastSeq;
    try {
      this.durableSub = subscriptionId(await this.ctx.request(aether.subscribe({ after, query: {}, deltas: false })));
    } catch (error) {
      if (after === null || !RESYNC_CODES.has(codeOf(error) ?? "")) throw error;
      this.ctx.store.updateAether(this.ctx.conn, (s) => ({ ...s, lastSeq: null }));
      this.durableSub = subscriptionId(await this.ctx.request(aether.subscribe({ after: null, query: {}, deltas: false })));
    }
    if (this.state.lastSeq === null) await this.readSnapshots();
    this.drain();
    this.scheduleStale();
    if (this.ctx.features.has("noesis")) await this.readNoesis();
    await this.syncDeltas();
    await Promise.all(this.watched.map((agent) => this.reread(agent)));
  }

  /** The link closed: in-flight streams and subscription ids are gone; cursors and transcripts stay. */
  stop(): void {
    this.running = false;
    this.durableSub = null;
    this.buffer = null;
    this.deltaSubs.clear();
    this.refetch.cancelAll();
    this.ctx.store.updateAether(this.ctx.conn, discardStreams);
  }

  onEvent(frame: Json): void {
    if (this.buffer) {
      this.buffer.push(frame);
      return;
    }
    if (frame.sub !== this.durableSub) return;
    this.apply(object(frame.event, "event frame") as unknown as aether.Event);
  }

  onDelta(frame: Json): void {
    const agent = string(frame.agent, "delta.agent");
    if (this.deltaSubs.get(agent) !== frame.sub) return;
    let outcome = "ignored";
    this.ctx.store.updateAether(this.ctx.conn, (s) => {
      const result = applyDelta(s, frame);
      outcome = result.outcome;
      return result.state;
    });
    if (outcome === "reread") void this.reread(agent);
  }

  /** §5.2.2: deltas only for the agents on screen, resubscribing as the set changes. */
  async watch(agents: readonly string[]): Promise<void> {
    const unique = [...new Set(agents.map((agent) => string(agent, "agent id")))];
    this.watched = unique.slice(0, MAX_DELTA_AGENTS);
    if (this.running && this.buffer === null) await this.syncDeltas();
  }

  get watching(): readonly string[] {
    return this.watched;
  }

  private apply(event: aether.Event): void {
    const before = this.state;
    this.ctx.store.updateAether(this.ctx.conn, (s) => applyEvent(s, event));
    const after = this.state;
    if (after === before) return;
    this.scheduleStale();
    if (after.noesis !== before.noesis) this.ctx.settingsChanged();
  }

  /** Refetches what applied events made stale; markers live in the store, so a reconnect resumes them. */
  private scheduleStale(): void {
    const { staleAgents, staleAttention } = this.state;
    for (const agent of staleAgents) this.refetch.schedule(`agent:${agent}`, () => void this.serial.run(`agent:${agent}`, () => this.refetchAgent(agent)));
    if (staleAttention) this.refetch.schedule("attention", () => void this.serial.run("attention", () => this.refetchAttention()));
    invariant(this.refetch.pending >= Math.min(1, staleAgents.length + Number(staleAttention)), "stale entities have a refetch scheduled");
  }

  private drain(): void {
    const buffered = this.buffer ?? [];
    this.buffer = null;
    for (const frame of buffered) this.onEvent(frame);
  }

  private async readSnapshots(): Promise<void> {
    const attention = replyObject(await this.ctx.request(aether.getAttention()), "attention") as unknown as aether.Attention;
    const watermark = seq(attention.last_seq, "attention.last_seq");
    const { agents, roots } = await this.readAgents();
    // Fresh snapshots supersede every stale marker; buffered events re-mark what they change.
    this.ctx.store.updateAether(this.ctx.conn, (s) => ({ ...s, attention, agents, roots, lastSeq: Math.max(watermark, s.lastSeq ?? 0), staleAgents: [], staleAttention: false }));
  }

  private async readAgents(): Promise<{ agents: Record<string, aether.Agent>; roots: string[] }> {
    const agents: Record<string, aether.Agent> = {};
    if (!this.ctx.features.has("agents_paging")) {
      const tree = replyObject(await this.ctx.request(aether.getTree({})), "tree");
      for (const node of Object.values(object(tree.nodes, "tree.nodes"))) {
        const agent = object(object(node, "tree node").agent, "tree node agent") as unknown as aether.Agent;
        agents[agent.id] = agent;
      }
      return { agents, roots: array(tree.roots, "tree.roots").map((id) => string(id, "root id")) };
    }
    let after: string | undefined;
    for (let page = 0; page < MAX_PAGES; page += 1) {
      const reply = replyObject(await this.ctx.request({ cmd: "list_agents_page", query: {}, limit: 500, ...(after ? { after } : {}) }), "agents_page");
      for (const value of array(reply.agents, "agents_page.agents")) {
        const agent = object(value, "agent") as unknown as aether.Agent;
        agents[agent.id] = agent;
      }
      if (typeof reply.next_after !== "string") break;
      invariant(reply.next_after !== after, "agent pages advance");
      after = reply.next_after;
    }
    const roots = Object.values(agents).filter((agent) => !agent.parent).map((agent) => agent.id);
    return { agents, roots };
  }

  private async readNoesis(): Promise<void> {
    const status = replyObject(await this.ctx.request({ cmd: "noesis_status" }), "noesis_status");
    this.ctx.store.updateAether(this.ctx.conn, (s) => ({ ...s, noesis: status }));
    this.ctx.settingsChanged();
  }

  /** Runs one reconciliation at a time, so overlapping watch calls cannot open untracked subscriptions. */
  private syncDeltas(): Promise<void> {
    const run = this.reconciling.then(() => this.reconcileDeltas());
    // The caller gets the failure; the chain stays usable for the next call.
    this.reconciling = run.catch(() => undefined);
    return run;
  }

  /** Drops unwatched subscriptions before adding any, re-reading the watched set after every await. */
  private async reconcileDeltas(): Promise<void> {
    for (let step = 0; step < MAX_DELTA_STEPS && this.running; step += 1) {
      const stale = [...this.deltaSubs].find(([agent]) => !this.watched.includes(agent));
      if (stale) {
        this.deltaSubs.delete(stale[0]);
        await this.ctx.request(aether.unsubscribe({ id: stale[1] }));
        continue;
      }
      const missing = this.watched.find((agent) => !this.deltaSubs.has(agent));
      if (missing === undefined) return;
      const sub = subscriptionId(await this.ctx.request(aether.subscribe({ after: null, query: { agent: missing }, deltas: true })));
      if (!this.running) return;
      this.deltaSubs.set(missing, sub);
      invariant(this.deltaSubs.size <= MAX_DELTA_AGENTS, "at most three delta subscriptions");
      void this.reread(missing);
    }
    invariant(!this.running || this.watched.every((agent) => this.deltaSubs.has(agent)), "delta subscriptions converge on the watched set");
  }

  /** Reads the transcript after its last read entry; asks during a read collapse into one more read. */
  reread(agent: string): Promise<void> {
    const running = this.rereading.get(agent);
    if (running) {
      this.again.add(agent);
      return running;
    }
    const read = this.readTranscript(agent)
      .then(() => {
        // A read that began before the latest ask cannot end recovery; the next one will.
        if (!this.again.has(agent)) this.ctx.store.updateAether(this.ctx.conn, (s) => endRecovery(s, agent));
      }, (error: unknown) => {
        this.ctx.store.report(`Could not read the transcript of ${agent}: ${errorText(error)}`, this.ctx.conn, codeOf(error));
        // Its streams stay in recovery until a read succeeds, so a timeout is read again.
        if (codeOf(error) === "timeout") this.again.add(agent);
      })
      .finally(() => {
        this.rereading.delete(agent);
        if (this.again.delete(agent) && this.running) void this.reread(agent);
      });
    this.rereading.set(agent, read);
    return read;
  }

  /** Pages forward from the read cursor, advancing it by what each page returned. */
  private async readTranscript(agent: string): Promise<void> {
    let after = this.state.transcripts[agent]?.lastEntrySeq ?? 0;
    for (let page = 0; page < MAX_PAGES; page += 1) {
      const { entries, more } = await this.transcriptPage(agent, after);
      const through = entries.reduce((max, entry) => Math.max(max, seq(entry.seq, "transcript entry seq")), after);
      invariant(entries.every((entry) => entry.seq > after), "a transcript page holds only entries after its cursor");
      this.ctx.store.updateAether(this.ctx.conn, (s) => ({ ...s, transcripts: { ...s.transcripts, [agent]: mergeEntries(s.transcripts[agent], entries, through) } }));
      if (!more || through <= after) return;
      after = through;
    }
  }

  private async transcriptPage(agent: string, after: number): Promise<{ entries: aether.TranscriptEntry[]; more: boolean }> {
    if (!this.ctx.features.has("transcript_paging")) {
      const data = array(replyData(await this.ctx.request(aether.getTranscript({ agent, after })), "transcript"), "transcript");
      return { entries: data as aether.TranscriptEntry[], more: false };
    }
    const page = replyObject(await this.ctx.request({ cmd: "get_transcript_page", agent, after, limit: 1000 }), "transcript_page");
    return { entries: array(page.entries, "transcript_page.entries") as aether.TranscriptEntry[], more: page.has_after === true };
  }

  /** Clears the marker first; a lost link puts it back for the next ready, a daemon answer does not. */
  private async refetchAgent(agent: string): Promise<void> {
    this.ctx.store.updateAether(this.ctx.conn, (s) => ({ ...s, staleAgents: s.staleAgents.filter((id) => id !== agent) }));
    try {
      const value = replyObject(await this.ctx.request(aether.getAgent({ id: agent })), "agent") as unknown as aether.Agent;
      invariant(value.id === agent, "get_agent answers the agent asked for");
      this.ctx.store.updateAether(this.ctx.conn, (s) => withAgent(s, value));
    } catch (error) {
      if (!LINK_LOST.has(codeOf(error) ?? "")) return this.ctx.store.report(`Could not refresh agent ${agent}: ${errorText(error)}`, this.ctx.conn, codeOf(error));
      this.ctx.store.updateAether(this.ctx.conn, (s) => (s.staleAgents.includes(agent) ? s : { ...s, staleAgents: [...s.staleAgents, agent] }));
      this.retryAfterTimeout(`agent ${agent}`, error);
    }
  }

  private async refetchAttention(): Promise<void> {
    this.ctx.store.updateAether(this.ctx.conn, (s) => ({ ...s, staleAttention: false }));
    try {
      const attention = replyObject(await this.ctx.request(aether.getAttention()), "attention") as unknown as aether.Attention;
      seq(attention.last_seq, "attention.last_seq");
      this.ctx.store.updateAether(this.ctx.conn, (s) => ({ ...s, attention }));
    } catch (error) {
      if (!LINK_LOST.has(codeOf(error) ?? "")) return this.ctx.store.report(`Could not refresh attention: ${errorText(error)}`, this.ctx.conn, codeOf(error));
      this.ctx.store.updateAether(this.ctx.conn, (s) => ({ ...s, staleAttention: true }));
      this.retryAfterTimeout("attention", error);
    }
  }

  /** A lost link refetches on the next ready; a timeout on a live one must be retried now, and seen. */
  private retryAfterTimeout(what: string, error: unknown): void {
    if (codeOf(error) !== "timeout" || !this.running) return;
    this.ctx.store.report(`Could not refresh ${what} in time; retrying: ${errorText(error)}`, this.ctx.conn, "timeout");
    this.scheduleStale();
  }
}

/** Installs a refetched agent row and keeps the root index in step with its parent. */
function withAgent(state: AetherConnState, agent: aether.Agent): AetherConnState {
  string(agent.id, "agent.id");
  const isRoot = !agent.parent;
  const listed = state.roots.includes(agent.id);
  const roots = isRoot && !listed ? [...state.roots, agent.id] : !isRoot && listed ? state.roots.filter((id) => id !== agent.id) : state.roots;
  invariant(roots.includes(agent.id) === isRoot, "an agent is a root exactly when it has no parent");
  return { ...state, agents: { ...state.agents, [agent.id]: agent }, roots };
}
