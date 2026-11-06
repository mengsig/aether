// A faux archond (protocol 24 with D1–D3's GitHub reads and D2's import) and a faux aetherd behind in-memory
// Transports, for the D4 tests and screenshot page. A test can fail or hold any command by name.

import type { Transport } from "@aether-ui/protocol";
import { invariant } from "../ref.ts";
import { item, link, type GithubWorld } from "./world.ts";

type Json = Record<string, unknown>;
type Answer = { ok: Json } | { err: Json } | "hold";

export const GITHUB_FEATURES = ["changes", "run_plan", "item_history", "item_filters", "github_links", "github_import", "device_roles"];

export interface FauxArchonOptions {
  readonly role?: "operator" | "observer";
  readonly features?: readonly string[];
}

interface Link { open: boolean; onFrame(frame: unknown): void }

const samePlace = (a: Json, b: Json): boolean =>
  String(a.host).toLowerCase() === String(b.host).toLowerCase() && String(a.repo).toLowerCase() === String(b.repo).toLowerCase() && a.number === b.number;

/** Answers with `reply` on a later turn, as a socket would; a held command is never answered. */
function answerLater(link: Link, command: Json, answer: Answer): void {
  if (answer === "hold") return;
  void Promise.resolve().then(() => {
    if (!link.open) return;
    const result = "err" in answer ? { Err: answer.err } : { Ok: answer.ok };
    link.onFrame(structuredClone({ frame: "reply", id: command.req, result }));
  });
}

export class FauxArchon {
  readonly sent: Json[] = [];
  watcher: Json = { enabled: true, poll_interval_secs: 120, revision: 1, mention_regex: "", label_regex: "", repositories: [
    { repository: "example-org/site", enabled: true, dispatched: 0, ignored: 0, target: { mode: "automatic" } },
  ] };
  readonly transport: Transport;
  /** Commands answered with an internal error, by name. */
  readonly failing = new Set<string>();
  /** Commands refused with archond's own wording, by name. */
  readonly refusing = new Map<string, string>();
  /** Commands left unanswered, by name. */
  readonly holding = new Set<string>();
  readonly world: GithubWorld;
  private readonly options: FauxArchonOptions;
  private link: Link | null = null;
  private seq = 100;

  constructor(world: GithubWorld, options: FauxArchonOptions = {}) {
    this.world = world;
    this.options = options;
    this.transport = {
      id: "ws-archon",
      open: async (_target, onFrame) => {
        const opened: Link = { open: true, onFrame };
        this.link = opened;
        return { send: (frame) => this.receive(opened, frame as Json), close: () => { opened.open = false; } };
      },
    };
  }

  commands(cmd: string): Json[] {
    return this.sent.filter((command) => command.cmd === cmd);
  }

  private receive(opened: Link, command: Json): void {
    this.sent.push(command);
    const name = String(command.cmd);
    if (this.holding.has(name)) return answerLater(opened, command, "hold");
    if (this.failing.has(name)) return answerLater(opened, command, { err: { code: "internal", message: `${name} failed in the faux archond` } });
    const refusal = this.refusing.get(name);
    if (refusal !== undefined) return answerLater(opened, command, { err: { code: "forbidden", message: refusal } });
    answerLater(opened, command, this.answer(command));
  }

  private answer(command: Json): Answer {
    const reply = (tag: string, data: unknown): Answer => ({ ok: { reply: tag, data } });
    switch (command.cmd) {
      case "hello": return reply("welcome", { protocol_version: 24, daemon_version: "1.5.0", pid: 7, min_protocol_version: 23,
        features: this.options.features ?? GITHUB_FEATURES, device: { id: "dev_1", name: "phone", role: this.options.role ?? "operator" } });
      case "subscribe_changes": return reply("subscribed", { id: 6, last_seq: this.seq });
      case "subscribe_feed": return reply("subscribed", { id: 5, last_seq: 10 });
      case "feed_history": return reply("feed", []);
      case "list_item_summaries": return reply("item_summaries_at", { summaries: [...this.world.items.values()].map((entry) => entry.summary), as_of: this.seq });
      case "get_item": {
        const entry = this.world.items.get(String(command.item));
        return entry ? reply("item_detail", entry.detail) : { err: { code: "not_found", message: `no item ${String(command.item)}` } };
      }
      case "item_links": return reply("item_links", this.itemLinks(String(command.item)));
      case "issue_items": {
        const at = command.ref as Json;
        return reply("issue_items", { links: this.world.links.filter((entry) => samePlace(entry.ref as Json, at) && (entry.ref as Json).kind === at.kind) });
      }
      case "github_watcher": return reply("github_watcher", this.watcher);
      case "list_github_objects": return reply("github_objects", { objects: this.world.objects.filter((entry) => String((entry.ref as Json).repo).toLowerCase() === String(command.repo).toLowerCase()), synced_at: "2026-11-04T09:55:00Z" });
      case "import_issue": return this.importIssue(command.ref as Json);
      default: return { err: { code: "unknown_command", message: `${String(command.cmd)} is not scripted` } };
    }
  }

  private itemLinks(id: string): Json {
    const links = this.world.links.filter((entry) => entry.item === id);
    const objects = this.world.objects.filter((entry) => links.some((at) => samePlace(at.ref as Json, entry.ref as Json)));
    return { links, objects };
  }

  /** Keyed by the issue, as D2 derives it: a repeat returns the first item. */
  private importIssue(at: Json): Answer {
    invariant(at.kind === "issue", "only an issue is imported");
    const id = `qi_import_${String(at.number)}`;
    const known = this.world.items.get(id);
    if (known) return { ok: { reply: "item", data: known.summary } };
    const cached = this.world.objects.find((entry) => samePlace(entry.ref as Json, at));
    const created = item(id, String(cached?.title ?? `Issue #${String(at.number)}`), { state: "queued" }, { issue: at });
    this.world.items.set(id, created);
    this.world.links.push(link(id, at, "imported", "import"));
    this.tell({ subject: "item", item: id, state: "queued" });
    this.tell({ subject: "link", item: id, host: at.host, repo: at.repo, number: at.number, relation: "imported" });
    return { ok: { reply: "item", data: created.summary } };
  }

  /** Streams a change, as archond does after each commit. */
  tell(subject: Json): void {
    const opened = this.link;
    if (!opened?.open) return;
    const frame = { frame: "change", sub: 6, change: { seq: ++this.seq, at: new Date().toISOString(), ...subject } };
    void Promise.resolve().then(() => { if (opened.open) opened.onFrame(frame); });
  }
}

/** A faux aetherd: it answers core's sync and lists the world's agents, nothing more. */
export class FauxAether {
  readonly transport: Transport;
  readonly instance: string;
  private readonly agents: readonly Json[];

  constructor(agents: readonly Json[], instance = "studio") {
    this.agents = agents;
    this.instance = instance;
    this.transport = {
      id: "ws-aether",
      open: async (_target, onFrame) => {
        const opened: Link = { open: true, onFrame };
        return { send: (frame) => answerLater(opened, frame as Json, this.answer(frame as Json)), close: () => { opened.open = false; } };
      },
    };
  }

  private answer(command: Json): Answer {
    const reply = (tag: string, data: unknown): Answer => ({ ok: { reply: tag, data } });
    switch (command.cmd) {
      case "hello": return reply("welcome", { protocol_version: 26, daemon_version: "1.3.0-faux", pid: 1, schema_version: 29, instance_id: this.instance,
        min_protocol_version: 17, submission_horizon: "hz_1", features: ["agents_paging", "transcript_paging"], idle_timeout_secs: null,
        device: { id: "dev_a", name: "phone", role: "operator" } });
      case "subscribe": return reply("subscribed", { id: 1, last_seq: 0 });
      case "unsubscribe": case "ping": return reply("ok", null);
      case "get_attention": return reply("attention", { agents: [], approvals: [], questions: [], last_seq: 0 });
      case "list_agents_page": return reply("agents_page", { agents: this.agents });
      default: return { err: { code: "invalid_argument", message: `the faux aetherd has no ${String(command.cmd)}` } };
    }
  }
}
