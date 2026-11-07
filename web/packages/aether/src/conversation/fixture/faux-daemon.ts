// A faux aetherd behind an in-memory Transport, for the conversation tests and the screenshot
// page: it answers the commands core's sync sends, keeps transcripts, and pushes events and deltas.

import type { aether, Transport } from "@aether-ui/protocol";
import { invariant } from "../../fleet/invariant.ts";

type Json = Record<string, unknown>;

interface Link { open: boolean; onFrame(frame: unknown): void; onClose(code: number, reason: string): void }

export const FAUX_FEATURES = ["deltas", "delta_order", "transcript_paging", "agents_paging"];

export interface FauxOptions {
  readonly instance: string;
  readonly agents: readonly aether.Agent[];
  /** Commands left unanswered while their name is here, to hold a reply mid-flight. */
  readonly hold?: Set<string>;
}

export class FauxDaemon {
  readonly sent: Json[] = [];
  readonly transport: Transport;
  readonly agents: Map<string, aether.Agent>;
  readonly hold: Set<string>;
  private readonly instance: string;
  private readonly transcripts = new Map<string, aether.TranscriptEntry[]>();
  private readonly events: aether.Event[] = [];
  /** Events committed without being pushed, as a lossy link would have lost them. */
  private readonly unsent: aether.Event[] = [];
  private readonly deltaSubs = new Map<string, number>();
  private durableSub: number | null = null;
  private nextSub = 1;
  private lastSeq = 0;
  private link: Link | null = null;

  constructor(options: FauxOptions) {
    this.instance = options.instance;
    this.agents = new Map(options.agents.map((agent) => [agent.id, agent]));
    this.hold = options.hold ?? new Set();
    this.transport = {
      id: "ws-aether",
      open: async (_target, onFrame, onClose) => {
        const link: Link = { open: true, onFrame, onClose };
        this.link = link;
        return { send: (frame) => this.receive(link, frame as Json), close: () => { link.open = false; } };
      },
    };
  }

  commands(cmd: string): Json[] {
    return this.sent.filter((command) => command.cmd === cmd);
  }

  /** Seeds a transcript without events, as history from before the client connected. */
  seed(agent: string, messages: readonly { message: aether.Message; invocation?: string }[]): void {
    const entries = this.transcript(agent);
    for (const { message, invocation } of messages) {
      this.lastSeq += 1;
      entries.push({ seq: this.lastSeq, at: stamp(this.lastSeq), invocation: invocation ?? null, message });
    }
  }

  /** Commits a message: a transcript entry and its `agent.message` event, pushed unless `silent`. */
  commit(agent: string, message: aether.Message, invocation: string | null = null, silent = false): aether.TranscriptEntry {
    invariant(this.agents.has(agent), `the faux daemon knows agent ${agent}`);
    const event = this.event(agent, { type: "agent.message", role: message.role, content: message.content, invocation }, silent);
    const entry: aether.TranscriptEntry = { seq: event.seq, at: event.at, invocation, message };
    this.transcript(agent).push(entry);
    return entry;
  }

  completed(agent: string, invocation: string, usage: aether.Usage, model: aether.ModelRef): void {
    this.event(agent, { type: "invocation.completed", invocation, usage, model, duration_ms: 4200, snapshot: "snap_faux" }, false);
  }

  /** Pushes a delta frame on the agent's delta subscription, if the client has one. */
  delta(agent: string, invocation: string, order: number, delta: aether.Delta): void {
    const sub = this.deltaSubs.get(agent);
    if (sub === undefined) return;
    this.push({ frame: "delta", sub, agent, invocation, invocation_seq: order, delta });
  }

  /** Pushes the events `commit(…, silent)` held back, late, on the durable subscription. */
  replayLast(agent: string): void {
    const late = this.unsent.splice(0).filter((event) => event.agent_id === agent);
    invariant(late.length > 0 && this.durableSub !== null, "held-back events and a durable subscription to send them on");
    for (const event of late) this.push({ frame: "event", sub: this.durableSub, event });
  }

  drop(code = 1006): void {
    const link = this.link;
    this.link = null;
    this.deltaSubs.clear();
    this.durableSub = null;
    if (!link?.open) return;
    link.open = false;
    link.onClose(code, "connection lost");
  }

  /** Answers the newest held command of this name now. */
  release(cmd: string): void {
    this.hold.delete(cmd);
    const command = this.commands(cmd).at(-1);
    invariant(command !== undefined && this.link !== null, `a held ${cmd} to release`);
    this.answer(this.link, command);
  }

  private transcript(agent: string): aether.TranscriptEntry[] {
    let entries = this.transcripts.get(agent);
    if (!entries) {
      entries = [];
      this.transcripts.set(agent, entries);
    }
    return entries;
  }

  private event(agent: string, kind: Json, silent: boolean): aether.Event {
    this.lastSeq += 1;
    const event = { seq: this.lastSeq, id: `evt_${this.lastSeq}`, at: stamp(this.lastSeq), actor: { actor: "system" }, agent_id: agent, kind } as unknown as aether.Event;
    this.events.push(event);
    if (silent) this.unsent.push(event);
    else if (this.durableSub !== null) this.push({ frame: "event", sub: this.durableSub, event });
    return event;
  }

  private push(frame: Json): void {
    if (this.link?.open) this.link.onFrame(structuredClone(frame));
  }

  private receive(link: Link, command: Json): void {
    this.sent.push(command);
    if (this.hold.has(String(command.cmd))) return;
    // Replies arrive on a later turn, as they would from a socket.
    void Promise.resolve().then(() => this.answer(link, command));
  }

  private answer(link: Link, command: Json): void {
    if (!link.open) return;
    const { reply, then } = this.reply(command);
    link.onFrame(structuredClone({ frame: "reply", id: command.req, result: reply }));
    for (const frame of then) link.onFrame(structuredClone(frame));
  }

  private reply(command: Json): { reply: Json; then: Json[] } {
    const ok = (tag: string, data: unknown) => ({ reply: { Ok: { reply: tag, data } }, then: [] as Json[] });
    switch (command.cmd) {
      case "hello": return ok("welcome", {
        protocol_version: 26, daemon_version: "1.3.0-faux", pid: 1, schema_version: 29, instance_id: this.instance,
        min_protocol_version: 17, features: FAUX_FEATURES, device: { id: "dev_faux", name: "faux", role: "operator" }, idle_timeout_secs: null,
      });
      case "subscribe": return this.subscribe(command);
      case "unsubscribe": return ok("ok", null);
      case "get_attention": return ok("attention", { agents: [], approvals: [], questions: [], last_seq: this.lastSeq });
      case "list_agents_page": return ok("agents_page", { agents: [...this.agents.values()] });
      case "get_agent": return ok("agent", this.agents.get(String(command.id)));
      case "get_transcript_page": return ok("transcript_page", this.page(command));
      default: return { reply: { Err: { code: "invalid_argument", message: `the faux daemon has no ${String(command.cmd)}` } }, then: [] };
    }
  }

  private subscribe(command: Json): { reply: Json; then: Json[] } {
    const id = this.nextSub++;
    const reply = { Ok: { reply: "subscribed", data: { id, last_seq: this.lastSeq } } };
    if (command.deltas === true) {
      this.deltaSubs.set(String((command.query as Json).agent), id);
      return { reply, then: [] };
    }
    this.durableSub = id;
    const after = typeof command.after === "number" ? command.after : null;
    const replay = after === null ? [] : this.events.filter((event) => event.seq > after).map((event) => ({ frame: "event", sub: id, event }));
    return { reply, then: replay };
  }

  private page(command: Json): Json {
    const entries = this.transcripts.get(String(command.agent)) ?? [];
    const after = typeof command.after === "number" ? command.after : 0;
    const limit = typeof command.limit === "number" ? command.limit : 200;
    invariant(command.before === undefined, "the faux daemon pages forward only");
    const start = firstAfter(entries, after);
    const slice = entries.slice(start, start + limit);
    return { entries: slice, has_before: start > 0, has_after: start + limit < entries.length, last_seq: this.lastSeq };
  }
}

/** The index of the first entry past `after`, by binary search over ascending seqs. */
function firstAfter(entries: readonly aether.TranscriptEntry[], after: number): number {
  let low = 0;
  let high = entries.length;
  while (low < high) {
    const mid = (low + high) >>> 1;
    if ((entries[mid]?.seq ?? 0) <= after) low = mid + 1;
    else high = mid;
  }
  return low;
}

const stamp = (n: number): string => new Date(Date.UTC(2026, 10, 4, 9, 0, 0) + n * 7_000).toISOString();
