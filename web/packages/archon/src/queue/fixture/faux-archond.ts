// A faux archond behind a Transport, for tests and the screenshot page. At protocol 24 it streams flattened
// Change records; at 23 it refuses the 24 hello, as an old archond does, and only feeds notifications.

import type { Transport } from "@aether-ui/protocol";
import { invariant } from "../model.ts";
import type { FauxItem, World } from "./world.ts";

type Json = Record<string, unknown>;
type Answer = { ok: Json } | { err: Json };

export const FEATURES_24 = ["changes", "run_plan", "item_history", "item_filters", "github_links", "device_roles"];

export interface FauxOptions {
  readonly protocol: 23 | 24;
  readonly role?: "operator" | "observer";
}

interface Link { open: boolean; onFrame(frame: unknown): void }

export class FauxArchond {
  readonly sent: Json[] = [];
  /** Commands left unanswered while their name is here, to hold a reply mid-flight. */
  readonly hold = new Set<string>();
  private readonly held: { readonly link: Link; readonly command: Json; readonly answer: Answer }[] = [];
  readonly world: World;
  readonly transport: Transport;
  private readonly options: FauxOptions;
  private link: Link | null = null;
  private seq = 100;
  private feedSeq = 10;
  private readonly changeSub = 6;
  private readonly feedSub = 5;

  constructor(world: World, options: FauxOptions) {
    invariant(options.protocol === 23 || options.protocol === 24, "a faux archond speaks protocol 23 or 24");
    this.world = world;
    this.options = options;
    this.transport = {
      id: "ws-archon",
      open: async (_target, onFrame) => {
        const link: Link = { open: true, onFrame };
        this.link = link;
        return { send: (frame) => this.receive(link, frame as Json), close: () => { link.open = false; } };
      },
    };
  }

  commands(cmd: string): Json[] {
    return this.sent.filter((command) => command.cmd === cmd);
  }

  /** Answers the oldest held command of this name now. */
  release(cmd: string): void {
    this.hold.delete(cmd);
    const index = this.held.findIndex((entry) => entry.command.cmd === cmd);
    invariant(index >= 0, `a held ${cmd} to release`);
    const [entry] = this.held.splice(index, 1);
    invariant(entry !== undefined, "the held entry was found");
    this.reply(entry.link, entry.command, entry.answer);
  }

  private receive(link: Link, command: Json): void {
    this.sent.push(command);
    const answer = this.answer(command);
    if (this.hold.has(String(command.cmd))) return void this.held.push({ link, command, answer });
    // Replies arrive on a later turn, as they would from a socket.
    void Promise.resolve().then(() => this.reply(link, command, answer));
  }

  private reply(link: Link, command: Json, answer: Answer): void {
    if (!link.open) return;
    const result = "err" in answer ? { Err: answer.err } : { Ok: answer.ok };
    link.onFrame({ frame: "reply", id: command.req, result: structuredClone(result) });
  }

  private entry(item: unknown): FauxItem | undefined {
    return typeof item === "string" ? this.world.items.get(item) : undefined;
  }

  private answer(command: Json): Answer {
    const reply = (tag: string, data: unknown): Answer => ({ ok: { reply: tag, data } });
    const missing = { err: { code: "not_found", message: `no such ${String(command.item ?? command.run ?? "entity")}` } };
    const v24 = this.options.protocol === 24;
    switch (command.cmd) {
      case "hello": return this.hello(command);
      case "subscribe_changes": return reply("subscribed", { id: this.changeSub, last_seq: this.seq });
      case "subscribe_feed": return reply("subscribed", { id: this.feedSub, last_seq: this.feedSeq });
      case "feed_history": return reply("feed", []);
      case "list_item_summaries": {
        // Protocol 23 summaries carry pr_url but not the 24 issue and pr refs.
        const rows = [...this.world.items.values()].map(({ summary }) => (v24 ? summary : Object.fromEntries(Object.entries(summary).filter(([key]) => key !== "issue" && key !== "pr"))));
        return command.with_as_of === true && v24 ? reply("item_summaries_at", { summaries: rows, as_of: this.seq }) : reply("item_summaries", rows);
      }
      case "get_item": { const entry = this.entry(command.item); return entry ? reply("item_detail", entry.detail) : missing; }
      case "status": return reply("status", this.status());
      case "repo_add": {
        const repo = { id: `repo_${this.world.repos.length}`, name: command.name, path: command.path, default_pipeline: command.default_pipeline, base_branch: command.base_branch, ship: "pr", added_at: new Date().toISOString() };
        this.world.repos.push(repo);
        return reply("repo", repo);
      }
      case "repo_list": return reply("repos", this.world.repos);
      case "list_questions": return reply("questions", this.world.questions);
      case "list_reports": return reply("reports", this.world.reports);
      case "list_pipelines": return reply("catalog", { pipelines: [{ name: "adaptive", stages: [] }, { name: "direct", stages: [] }], problems: [] });
      case "cost_report": return reply("cost_report", { by: "repo", estimates_counted: false, range: { since: "2026-10-29", until: "2026-11-04" },
        total: { counted_usd: 7.49, cost_usd: 7.49, estimated_cost_usd: 0 }, rows: [{ key: "repo_site", counted_usd: 6.42, cost_usd: 6.42, estimated_cost_usd: 0 }, { key: "repo_api", counted_usd: 1.07, cost_usd: 1.07, estimated_cost_usd: 0 }] });
      case "list_artifacts": {
        const entry = this.entry(command.item);
        return entry ? reply("artifacts", { item: command.item, dir: "/artifacts", omitted: 0, total_bytes: 0, files: entry.artifacts.map(({ name, bytes }) => ({ name, bytes })) }) : missing;
      }
      case "read_artifact": {
        const file = this.entry(command.item)?.artifacts.find((artifact) => artifact.name === command.name);
        if (!file) return missing;
        const text = String(file.text);
        const offset = Number(command.offset ?? 0);
        const end = Math.min(text.length, offset + Number(command.limit ?? text.length));
        return reply("artifact_page", { item: command.item, name: file.name, path: `/artifacts/${String(file.name)}`, offset,
          text: text.slice(offset, end), total_bytes: text.length, next_offset: end < text.length ? end : null });
      }
      case "run_plan": { if (!v24) break; const plan = this.world.plans.get(String(command.run)); return plan ? reply("run_plan", plan) : missing; }
      case "item_links": { if (!v24) break; const entry = this.entry(command.item); return entry ? reply("item_links", entry.links) : missing; }
      default: return this.change(command);
    }
    return { err: { code: "unknown_command", message: `${String(command.cmd)} is not in protocol ${this.options.protocol}` } };
  }

  private hello(command: Json): Answer {
    if (this.options.protocol === 23 && command.protocol_version !== 23) return { err: { code: "incompatible", message: "archond speaks protocol 23; reload the app" } };
    const welcome: Json = { protocol_version: this.options.protocol, daemon_version: this.options.protocol === 24 ? "1.5.0" : "1.4.12", pid: 7 };
    if (this.options.protocol === 24) Object.assign(welcome, { min_protocol_version: 23, features: FEATURES_24, device: { id: "dev_1", name: "phone", role: this.options.role ?? "operator" } });
    return { ok: { reply: "welcome", data: welcome } };
  }

  private status(): Json {
    const states = [...this.world.items.values()].map((entry) => (entry.summary.state as Json).state);
    return {
      daemon_version: "1.5.0", pause: this.world.pause, queued_items: states.filter((s) => s === "queued").length, running_agents: 3, active_runs: 2,
      blocked_items: states.filter((s) => s === "blocked").length, open_questions: this.world.questions.filter((q) => !q.answer).length, open_gates: 1,
      spend_today_usd: 4.25, spend_total_usd: 61.9, caps: { budgets_enabled: true, budget_usd_daily: 20, budget_usd_total: 500, max_parallel_agents: 4, max_active_runs: 3 },
    };
  }

  /** The changes the screens send; each one tells the client as this protocol would. */
  private change(command: Json): Answer {
    const ok: Answer = { ok: { reply: "ok", data: null } };
    switch (command.cmd) {
      case "cancel_item": return this.move(command.item, { state: "cancelled", reason: command.reason ?? null });
      case "retry_item": return this.move(command.item, { state: "queued" });
      case "pause": case "resume": {
        const on = command.cmd === "pause";
        const pause = this.world.pause;
        if (command.scope === "global") pause.global = on;
        else {
          const list = command.scope === "repo" ? pause.repos : pause.pipelines;
          const name = String(command.name);
          if (on && !list.includes(name)) list.push(name);
          if (!on) list.splice(list.indexOf(name), list.includes(name) ? 1 : 0);
        }
        return ok;
      }
      case "answer": {
        const question = this.world.questions.find((q) => q.id === command.question);
        if (!question) return { err: { code: "not_found", message: "no such question" } };
        Object.assign(question, { answer: command.text, answered_at: new Date().toISOString() });
        this.tell(typeof question.item === "string" ? question.item : null, { subject: "question", question: question.id, item: question.item, open: false });
        return { ok: { reply: "question", data: question } };
      }
      case "say": this.world.said.push(String(command.text)); return { ok: { reply: "message_receipt", data: { seq: this.world.said.length } } };
      case "enqueue": return this.enqueue(command);
      default: return { err: { code: "unknown_command", message: `${String(command.cmd)} is not scripted` } };
    }
  }

  private move(id: unknown, state: Json): Answer {
    const entry = this.entry(id);
    if (!entry) return { err: { code: "not_found", message: `no item ${String(id)}` } };
    entry.summary = { ...entry.summary, state, waiting_reason: null, waiting_on: null, updated_at: new Date().toISOString() };
    entry.detail = { ...entry.detail, item: { ...(entry.detail.item as Json), state }, waiting_reason: null, waiting_on: null };
    this.tell(String(id), { subject: "item", item: id, state: state.state });
    return { ok: { reply: "item", data: entry.detail.item } };
  }

  private enqueue(command: Json): Answer {
    invariant(typeof command.title === "string" && typeof command.brief === "string", "enqueue carries a title and brief");
    const id = `qi_new_${this.world.items.size}`;
    const now = new Date().toISOString();
    const summary: Json = { id, title: command.title, brief: command.brief, state: { state: "queued" }, repo: command.repo ?? null, pipeline: command.pipeline ?? "adaptive",
      priority: command.priority ?? "normal", created_at: now, updated_at: now, dependencies: [], tags: [], model_plan: {}, spent_usd: 0 };
    this.world.items.set(id, { summary, detail: { item: summary, runs: [], stages: [], gates: [], spent_usd: 0 }, links: { links: [], objects: [] }, artifacts: [] });
    this.tell(id, { subject: "item", item: id, state: "queued" });
    return { ok: { reply: "item", data: summary } };
  }

  /** A change frame at protocol 24, a feed notification at 23. */
  tell(item: string | null, subject: Json): void {
    const link = this.link;
    if (!link?.open) return;
    const frame = this.options.protocol === 24
      ? { frame: "change", sub: this.changeSub, change: { seq: ++this.seq, at: new Date().toISOString(), ...subject } }
      : { frame: "feed", sub: this.feedSub, item: { id: `fd_${++this.feedSeq}`, seq: this.feedSeq, at: new Date().toISOString(), entry: { entry: "notification", kind: "progress", item, text: "changed" } } };
    void Promise.resolve().then(() => { if (link.open) link.onFrame(frame); });
  }
}
