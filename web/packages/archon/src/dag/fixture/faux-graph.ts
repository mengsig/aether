// Wraps the C1 faux archond with the protocol 24 read models the C2 screens use: `task_graph`,
// `item_history`, `get_initiative` and the `list_items` filters. Everything else goes to the C1 faux.
// It watches the frames it passes on, so `as_of` is the last change seq the client was sent.

import type { Transport } from "@aether-ui/protocol";
import { invariant } from "../../queue/model.ts";
import { FauxArchond, type FauxOptions } from "../../queue/fixture/faux-archond.ts";
import type { World } from "../../queue/fixture/world.ts";

type Json = Record<string, unknown>;

export interface GraphWorld {
  readonly base: World;
  /** Edges besides the items' own `dependencies`, which become `depends_on` edges. */
  readonly extraEdges: readonly { from: string; to: string; kind: "initiative_after" | "stacked_on" }[];
  readonly initiatives: Json[];
  /** Recorded transitions by item; an item without any has the one transition of its creation. */
  readonly histories: Map<string, Json[]>;
}

const MINE: ReadonlySet<string> = new Set(["task_graph", "item_history", "get_initiative", "list_items"]);

/** A summary's state kind: a bare kind or an object tagged by `state`. */
export function kindOf(summary: Json): string {
  const state = summary.state;
  return typeof state === "string" ? state : String((state as Json).state);
}

/** The `TaskNode` archond derives from an item summary (CONTRACTS.md §1.3). */
export function taskNodeOf(summary: Json): Json {
  const state = summary.state as Json;
  const reason = summary.waiting_reason as Json | null;
  const detail = typeof state.reason === "string" ? state.reason : typeof state.error === "string" ? state.error : null;
  return {
    id: summary.id, repo: summary.repo ?? null, title: summary.title, pipeline: summary.pipeline, priority: summary.priority, state: kindOf(summary),
    state_detail: detail, stage: summary.stage ?? null, waiting_reason: reason?.code ?? null, spent_usd: summary.spent_usd ?? 0,
    initiative: summary.initiative ?? null, tags: summary.tags ?? [], created_at: summary.created_at, updated_at: summary.updated_at,
    issue: summary.issue ?? null, pr: summary.pr ?? null, agents_running: kindOf(summary) === "running" ? 1 : 0,
  };
}

export class FauxGraphArchond {
  readonly inner: FauxArchond;
  readonly world: GraphWorld;
  readonly transport: Transport;
  private lastSeq = 0;
  private link: { open: boolean; onFrame(frame: unknown): void } | null = null;

  constructor(world: GraphWorld, options: FauxOptions) {
    invariant(options.protocol === 24, "the C2 read models exist at protocol 24 only");
    this.world = world;
    this.inner = new FauxArchond(world.base, options);
    this.transport = {
      id: "ws-archon",
      open: async (target, onFrame, onClose) => {
        const link = { open: true, onFrame };
        this.link = link;
        const inner = await this.inner.transport.open(target, (frame) => onFrame(this.watch(frame as Json)), onClose);
        return {
          send: (frame) => {
            const command = frame as Json;
            if (!MINE.has(String(command.cmd))) return inner.send(frame);
            this.inner.sent.push(command);
            const result = this.answer(command);
            void Promise.resolve().then(() => { if (link.open) link.onFrame({ frame: "reply", id: command.req, result: structuredClone(result) }); });
          },
          close: () => { link.open = false; inner.close(); },
        };
      },
    };
  }

  /** Moves an item to `state` as archond would: its summary, its history, then a change frame. */
  move(id: string, state: Json, by = "daemon"): void {
    const entry = this.world.base.items.get(id);
    invariant(entry !== undefined, `move names a world item, not ${id}`);
    const from = kindOf(entry.summary);
    const at = new Date(Date.parse(String(entry.summary.updated_at)) + 60_000).toISOString();
    entry.summary = { ...entry.summary, state, waiting_reason: null, waiting_on: null, updated_at: at };
    const history = this.historyOf(id);
    history.push({ seq: this.lastSeq + 1, at, from, to: state.state, reason: null, by });
    this.inner.tell(id, { subject: "item", item: id, state: state.state });
  }

  private historyOf(id: string): Json[] {
    let history = this.world.histories.get(id);
    if (history === undefined) {
      const summary = this.world.base.items.get(id)?.summary;
      invariant(summary !== undefined, `history is kept for world items, not ${id}`);
      history = [{ seq: 1, at: summary.created_at, from: null, to: "queued", reason: null, by: "cli" }];
      this.world.histories.set(id, history);
    }
    return history;
  }

  /** Feature `task_graph` joins the welcome; subscription replies and change frames move `as_of`. */
  private watch(frame: Json): Json {
    if (frame.frame === "change") this.lastSeq = Math.max(this.lastSeq, Number((frame.change as Json).seq));
    const ok = (frame.result as Json | undefined)?.Ok as Json | undefined;
    if (ok?.reply === "subscribed") this.lastSeq = Math.max(this.lastSeq, Number((ok.data as Json).last_seq ?? 0));
    if (ok?.reply !== "welcome") return frame;
    const data = ok.data as Json;
    const features = [...(data.features as string[]), "task_graph"];
    return { ...frame, result: { Ok: { ...ok, data: { ...data, features } } } };
  }

  private answer(command: Json): Json {
    const reply = (tag: string, data: unknown): Json => ({ Ok: { reply: tag, data } });
    switch (command.cmd) {
      case "task_graph": return reply("task_graph", this.graph(Number(command.limit ?? 500)));
      case "item_history": {
        const id = String(command.item);
        if (!this.world.base.items.has(id)) return { Err: { code: "not_found", message: `no item ${id}` } };
        return reply("item_history", { item: id, transitions: this.historyOf(id).slice(-Number(command.limit ?? 1000)) });
      }
      case "get_initiative": {
        const initiative = this.world.initiatives.find((entry) => entry.id === command.initiative);
        if (!initiative) return { Err: { code: "not_found", message: `no initiative ${String(command.initiative)}` } };
        const members = [...this.world.base.items.values()].filter((entry) => entry.summary.initiative === initiative.id).map((entry) => entry.detail.item);
        return reply("initiative", { initiative, members });
      }
      default: return reply("items", this.listItems(command));
    }
  }

  private graph(limit: number): Json {
    invariant(limit >= 1 && limit <= 2000, "task_graph limits are 1 to 2000");
    const summaries = [...this.world.base.items.values()].map((entry) => entry.summary)
      .sort((a, b) => String(a.created_at).localeCompare(String(b.created_at)) || String(a.id).localeCompare(String(b.id)));
    const nodes = summaries.slice(0, limit).map(taskNodeOf);
    const ids = new Set(nodes.map((node) => node.id as string));
    const kinds = new Map(summaries.map((summary) => [summary.id as string, kindOf(summary)]));
    const typed = [
      ...summaries.flatMap((summary) => ((summary.dependencies as string[] | undefined) ?? []).map((from) => ({ from, to: summary.id as string, kind: "depends_on" }))),
      ...this.world.extraEdges,
    ];
    const edges = typed.filter((edge) => ids.has(edge.from) && ids.has(edge.to))
      .map((edge) => ({ ...edge, satisfied: kinds.get(edge.from) === "succeeded" }))
      .sort((a, b) => a.from.localeCompare(b.from) || a.to.localeCompare(b.to) || a.kind.localeCompare(b.kind));
    const initiatives = this.world.initiatives.map(({ id, title, state, atomic }) => ({ id, title, state, atomic: atomic === true }));
    return { nodes, edges, initiatives, as_of: this.lastSeq, truncated: summaries.length > limit };
  }

  private listItems(command: Json): Json[] {
    const text = typeof command.text === "string" ? command.text.toLowerCase() : null;
    const since = typeof command.updated_since === "string" ? Date.parse(command.updated_since) : -Infinity;
    const before = typeof command.before === "string" ? Date.parse(command.before) : Infinity;
    const rows = [...this.world.base.items.values()].filter(({ summary }) => {
      const updated = Date.parse(String(summary.updated_at));
      const words = `${String(summary.title)}\n${String(summary.brief)}`.toLowerCase();
      return updated >= since && updated < before && (text === null || words.includes(text));
    });
    return rows.map((entry) => ({ ...(entry.detail.item as Json), state: entry.summary.state, updated_at: entry.summary.updated_at }))
      .sort((a, b) => String(b.updated_at).localeCompare(String(a.updated_at)))
      .slice(0, Number(command.limit ?? 100));
  }

  /** Whether the client is connected; a test asserts it before moving the world. */
  get connected(): boolean {
    return this.link?.open === true;
  }
}
