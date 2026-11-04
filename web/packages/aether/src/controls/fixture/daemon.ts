// A scripted aetherd behind an in-memory Transport, for the B3 screens' tests and screenshot page.
// It answers core's sync itself; each test scripts the rest by command name, and can hold a reply
// or drop the link mid-request to lose one.

import type { aether, Transport } from "@aether-ui/protocol";
import { invariant } from "../../fleet/invariant.ts";

type Json = Record<string, unknown>;

/** A reply's tag and data, a daemon error, or `"hold"` to leave the request unanswered. */
export type Answer = { readonly tag: string; readonly data: unknown } | { readonly code: string; readonly message: string } | "hold";
export type Script = Readonly<Record<string, (command: Json) => Answer>>;

export type Role = "operator" | "observer" | "local";

export interface ScriptOptions {
  readonly instance?: string;
  /** `local` opens the link as the desktop's `ipc` transport, which core reads as the host's own standing. */
  readonly role?: Role;
  readonly agents?: readonly aether.Agent[];
  readonly attention?: Partial<aether.Attention>;
  readonly events?: readonly aether.Event[];
  readonly features?: readonly string[];
  readonly script?: Script;
}

interface Link { open: boolean; onFrame(frame: unknown): void; onClose(code: number, reason: string): void }

export class ScriptedDaemon {
  readonly sent: Json[] = [];
  readonly transport: Transport;
  script: Script;
  attention: aether.Attention;
  private readonly options: ScriptOptions;
  private readonly held: Json[] = [];
  private link: Link | null = null;
  private durable: number | null = null;
  private events: aether.Event[];

  constructor(options: ScriptOptions = {}) {
    this.options = options;
    this.script = options.script ?? {};
    this.events = [...(options.events ?? [])];
    this.attention = { agents: [], approvals: [], questions: [], last_seq: this.lastSeq(), ...options.attention };
    this.transport = {
      id: options.role === "local" ? "ipc" : "ws-aether",
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

  /** Commits an event and pushes it on the durable subscription. */
  emit(agent: string | null, kind: Json): aether.Event {
    const seq = this.lastSeq() + 1;
    const event = { seq, id: `evt_${seq}`, at: stamp(seq), actor: { actor: "system" }, agent_id: agent, kind } as unknown as aether.Event;
    this.events.push(event);
    this.attention = { ...this.attention, last_seq: seq };
    if (this.durable !== null) this.push({ frame: "event", sub: this.durable, event });
    return event;
  }

  /** Closes the link as a lost network would: every unanswered request fails as disconnected. */
  drop(code = 1006): void {
    const link = this.link;
    this.link = null;
    this.durable = null;
    if (!link?.open) return;
    link.open = false;
    link.onClose(code, "connection lost");
  }

  /** Answers the oldest held request now, with what the script says at this moment. */
  release(): void {
    const command = this.held.shift();
    invariant(command !== undefined && this.link !== null, "a held request to release");
    this.answer(this.link, command, true);
  }

  private lastSeq(): number {
    return this.events.at(-1)?.seq ?? 0;
  }

  private push(frame: Json): void {
    if (this.link?.open) this.link.onFrame(structuredClone(frame));
  }

  private receive(link: Link, command: Json): void {
    this.sent.push(command);
    // Replies arrive on a later turn, as they would from a socket.
    void Promise.resolve().then(() => this.answer(link, command, false));
  }

  private answer(link: Link, command: Json, released: boolean): void {
    if (!link.open) return;
    const answer = this.scripted(command) ?? this.builtIn(command);
    if (answer === "hold" && !released) {
      this.held.push(command);
      return;
    }
    if (answer === "hold") throw new Error(`the script still holds ${String(command.cmd)}`);
    const result = "tag" in answer ? { Ok: { reply: answer.tag, data: answer.data } } : { Err: { code: answer.code, message: answer.message } };
    link.onFrame(structuredClone({ frame: "reply", id: command.req, result }));
    if (command.cmd === "subscribe" && command.deltas !== true) {
      const after = typeof command.after === "number" ? command.after : null;
      for (const event of after === null ? [] : this.events.filter((e) => e.seq > after)) this.push({ frame: "event", sub: this.durable, event });
    }
  }

  private scripted(command: Json): Answer | null {
    const handler = this.script[String(command.cmd)];
    return handler ? handler(command) : null;
  }

  private builtIn(command: Json): Answer {
    switch (command.cmd) {
      case "hello": return { tag: "welcome", data: this.welcome() };
      case "subscribe": {
        const id = command.deltas === true ? 2 : 1;
        if (command.deltas !== true) this.durable = id;
        return { tag: "subscribed", data: { id, last_seq: this.lastSeq() } };
      }
      case "unsubscribe": case "ping": return { tag: "ok", data: null };
      case "get_attention": return { tag: "attention", data: this.attention };
      case "list_agents_page": return { tag: "agents_page", data: { agents: this.options.agents ?? [] } };
      case "get_agent": return { tag: "agent", data: (this.options.agents ?? []).find((a) => a.id === command.id) };
      case "get_transcript_page": return { tag: "transcript_page", data: { entries: [], has_before: false, has_after: false, last_seq: this.lastSeq() } };
      default: return { code: "invalid_argument", message: `the scripted daemon has no ${String(command.cmd)}` };
    }
  }

  private welcome(): Json {
    const role = this.options.role ?? "operator";
    return {
      protocol_version: 26, daemon_version: "1.3.0-faux", pid: 1, schema_version: 29, instance_id: this.options.instance ?? "b3",
      min_protocol_version: 17, features: this.options.features ?? ["agents_paging", "transcript_paging", "deltas", "delta_order"],
      idle_timeout_secs: null, ...(role === "local" ? {} : { device: { id: "dev_faux", name: "Faux phone", role } }),
    };
  }
}

export const stamp = (n: number): string => new Date(Date.UTC(2026, 10, 4, 9, 0, 0) + n * 7_000).toISOString();
