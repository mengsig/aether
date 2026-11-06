// A faux aetherd that serves Noesis, behind an in-memory Transport, for the learning-screen tests and
// the screenshot page. Answer keys stay here until an answer is recorded, as the daemon seals them.

import type { aether, NoesisSettingsPatch, PrivilegedAction, Transport } from "@aether-ui/protocol";
import { invariant } from "../model.ts";

type Json = Record<string, unknown>;
type Result = { readonly reply: Json; readonly then: readonly Json[] };

interface Link { open: boolean; onFrame(frame: unknown): void; onClose(code: number, reason: string): void }

/** A question with the key the daemon keeps sealed until the learner answers. */
export interface HeldQuestion {
  readonly question: aether.Question;
  readonly key: aether.AnswerKey;
  readonly explanation: string;
}

export interface FauxNoesisOptions {
  readonly status: aether.NoesisStatus;
  readonly sessions?: readonly aether.NoesisSession[];
  readonly transcripts?: Readonly<Record<string, readonly aether.Message[]>>;
  readonly graph?: { readonly nodes: readonly aether.Proposition[]; readonly chains: readonly aether.Chain[] };
  readonly concepts?: readonly aether.ConceptState[];
  /** `ipc` makes the page the daemon's own host, which may write host-only settings. */
  readonly transport?: "ipc" | "ws-aether";
  /** The message `noesis_create_session` refuses with, as when no model may be used. */
  readonly refuse?: string;
  /** What `list_models` and `auth_status` answer; none by default. */
  readonly models?: readonly aether.ModelInfo[];
  readonly auth?: readonly aether.AuthInfo[];
}

const OK = (tag: string, data: unknown): Result => ({ reply: { Ok: { reply: tag, data } }, then: [] });
const ERR = (code: string, message: string): Result => ({ reply: { Err: { code, message } }, then: [] });
const SERVICE = new Set(["noesis_status", "noesis_configure"]);

export class FauxNoesis {
  readonly sent: Json[] = [];
  /** The privileged actions the desktop's main ran for the page, confirmed. */
  readonly privileged: PrivilegedAction[] = [];
  readonly transport: Transport;
  status: aether.NoesisStatus;
  refuse: string | undefined;
  models: readonly aether.ModelInfo[];
  auth: readonly aether.AuthInfo[];
  readonly sessions = new Map<string, aether.NoesisSession>();
  readonly held = new Map<string, HeldQuestion>();
  graph: { nodes: aether.Proposition[]; chains: aether.Chain[] };
  concepts: aether.ConceptState[];
  private readonly agents = new Map<string, aether.Agent>();
  private readonly transcripts = new Map<string, aether.TranscriptEntry[]>();
  private durableSub: number | null = null;
  private nextSub = 1;
  private seq = 0;
  private link: Link | null = null;

  constructor(options: FauxNoesisOptions) {
    this.status = options.status;
    this.refuse = options.refuse;
    this.models = options.models ?? [];
    this.auth = options.auth ?? [];
    this.graph = { nodes: [...(options.graph?.nodes ?? [])], chains: [...(options.graph?.chains ?? [])] };
    this.concepts = [...(options.concepts ?? [])];
    for (const session of options.sessions ?? []) this.addSession(session);
    for (const [agent, messages] of Object.entries(options.transcripts ?? {})) for (const message of messages) this.append(agent, message);
    this.transport = {
      id: options.transport ?? "ws-aether",
      open: async (_target, onFrame, onClose) => {
        const link: Link = { open: true, onFrame, onClose };
        this.link = link;
        return { send: (frame) => this.receive(link, frame as Json), close: () => { link.open = false; } };
      },
    };
  }

  /** The commands sent, by name: everything a screen asked the daemon for. */
  names(): string[] {
    return this.sent.map((command) => String(command.cmd));
  }

  /** The teacher speaks: a committed assistant message on the session's agent, pushed as an event. */
  teach(session: string, text: string): void {
    const agent = this.session(session).agent;
    const entry = this.append(agent, { role: "assistant", content: [{ type: "text", text }] });
    this.event(agent, { type: "agent.message", role: "assistant", content: entry.message.content, invocation: null });
  }

  /** Poses a question: the session's open question, a `noesis.question_posed` event, and its key held back. */
  pose(session: string, held: HeldQuestion): void {
    const question = { ...held.question, posed_at: stamp(this.seq + 1) };
    this.held.set(question.id, { ...held, question });
    this.update(session, { open_question: question });
    this.event(this.session(session).agent, { type: "noesis.question_posed", session, question });
  }

  problem(session: string, posed: aether.Problem): void {
    const problem = { ...posed, posed_at: stamp(this.seq + 1) };
    this.update(session, { active_problem: problem, phase: "practice" });
    this.event(this.session(session).agent, { type: "noesis.problem_posed", session, problem });
  }

  conclude(session: string, conclusion: Omit<aether.EventKindNoesisPracticeConcluded, "type" | "session">): void {
    this.update(session, { active_problem: null });
    this.event(this.session(session).agent, { type: "noesis.practice_concluded", session, ...conclusion });
  }

  drop(code = 1006): void {
    const link = this.link;
    this.link = null;
    this.durableSub = null;
    if (!link?.open) return;
    link.open = false;
    link.onClose(code, "connection lost");
  }

  private session(id: string): aether.NoesisSession {
    const session = this.sessions.get(id);
    invariant(session !== undefined, `the faux daemon has session ${id}`);
    return session;
  }

  private addSession(session: aether.NoesisSession): void {
    invariant(!this.sessions.has(session.id), `session ${session.id} is new`);
    this.sessions.set(session.id, session);
    this.agents.set(session.agent, noesisAgent(session.agent));
    if (session.open_question) this.held.set(session.open_question.id, { question: session.open_question, key: [], explanation: "" });
  }

  private update(id: string, patch: Partial<aether.NoesisSession>): void {
    this.seq += 1;
    this.sessions.set(id, { ...this.session(id), ...patch, updated_at: stamp(this.seq) });
  }

  private append(agent: string, message: aether.Message): aether.TranscriptEntry {
    this.seq += 1;
    const entry: aether.TranscriptEntry = { seq: this.seq, at: stamp(this.seq), invocation: null, message };
    const entries = this.transcripts.get(agent) ?? [];
    entries.push(entry);
    this.transcripts.set(agent, entries);
    return entry;
  }

  private event(agent: string | null, kind: Json): void {
    this.seq += 1;
    const event = { seq: this.seq, id: `evt_${this.seq}`, at: stamp(this.seq), actor: { actor: "system" }, agent_id: agent, kind };
    if (this.durableSub !== null) this.push({ frame: "event", sub: this.durableSub, event });
  }

  private push(frame: Json): void {
    if (this.link?.open) this.link.onFrame(structuredClone(frame));
  }

  private receive(link: Link, command: Json): void {
    this.sent.push(command);
    // Replies arrive on a later turn, as they would from a socket.
    void Promise.resolve().then(() => {
      if (!link.open) return;
      const { reply, then } = this.reply(command);
      link.onFrame(structuredClone({ frame: "reply", id: command.req, result: reply }));
      for (const frame of then) link.onFrame(structuredClone(frame));
    });
  }

  private reply(command: Json): Result {
    const cmd = String(command.cmd);
    if (cmd.startsWith("noesis_")) return this.noesis(cmd, command);
    switch (cmd) {
      case "hello": return OK("welcome", {
        protocol_version: 26, daemon_version: "1.3.0-faux", pid: 1, schema_version: 29, instance_id: "faux-noesis", min_protocol_version: 17,
        features: ["deltas", "delta_order", "transcript_paging", "agents_paging", "noesis"], device: { id: "dev_faux", name: "faux", role: "operator" }, idle_timeout_secs: null,
      });
      case "subscribe": return this.subscribe(command);
      case "unsubscribe": return OK("ok", null);
      case "get_attention": return OK("attention", { agents: [], approvals: [], questions: [], last_seq: this.seq });
      case "list_agents_page": return OK("agents_page", { agents: [...this.agents.values()] });
      case "get_agent": return this.agents.has(String(command.id)) ? OK("agent", this.agents.get(String(command.id))) : ERR("not_found", "agent");
      case "get_transcript_page": return OK("transcript_page", this.page(command));
      case "list_models": return OK("models", this.models);
      case "auth_status": return OK("auth", this.auth);
      default: return ERR("invalid_argument", `the faux daemon has no ${cmd}`);
    }
  }

  private subscribe(command: Json): Result {
    const id = this.nextSub++;
    if (command.deltas !== true) this.durableSub = id;
    return OK("subscribed", { id, last_seq: this.seq });
  }

  private page(command: Json): Json {
    const entries = this.transcripts.get(String(command.agent)) ?? [];
    const after = typeof command.after === "number" ? command.after : 0;
    const slice = entries.filter((entry) => entry.seq > after);
    return { entries: slice, has_before: false, has_after: false, last_seq: this.seq };
  }

  private noesis(cmd: string, command: Json): Result {
    if (!this.status.enabled && !SERVICE.has(cmd)) return ERR("noesis_disabled", "Noesis is disabled");
    switch (cmd) {
      case "noesis_status": return OK("noesis_status", this.status);
      case "noesis_configure": return this.configure(command.patch as aether.NoesisSettingsPatch);
      case "noesis_list_sessions": return OK("noesis_sessions", { sessions: [...this.sessions.values()] });
      case "noesis_get_session": {
        const session = this.sessions.get(String(command.session));
        return session ? OK("noesis_session", session) : ERR("not_found", `session \`${String(command.session)}\``);
      }
      case "noesis_create_session": return this.create(command);
      case "noesis_send": return OK("ok", null);
      case "noesis_answer": return this.answer(command);
      case "noesis_delete_session": this.sessions.delete(String(command.session)); return OK("ok", null);
      case "noesis_extract": return OK("noesis_job", { job: { id: "job_faux", state: "queued" } });
      case "noesis_graph": return OK("noesis_graph", { ...this.graphSlice(command.query), clusters: [], truncated: false });
      case "noesis_learner": return OK("noesis_learner", { concepts: this.concepts });
      case "noesis_delete_proposition": return this.deleteProposition(String(command.proposition));
      case "noesis_delete_source": return OK("ok", null);
      default: return ERR("invalid_argument", `the faux daemon has no ${cmd}`);
    }
  }

  /** What desktop main does with `noesis.configure`: its flat §6.1 keys sent nested, the status back. */
  runPrivileged(action: PrivilegedAction): unknown {
    invariant(action.kind === "noesis.configure", `the faux main runs noesis.configure, not ${action.kind}`);
    invariant(Object.keys(action.patch).length > 0, "a privileged patch names a key");
    this.privileged.push(structuredClone(action));
    const reply = this.configure(nestedPatch(action.patch)).reply as { Ok: { data: unknown } };
    return reply.Ok.data;
  }

  private configure(patch: aether.NoesisSettingsPatch): Result {
    const settings = mergeSettings(this.status.settings, patch);
    this.status = { ...this.status, enabled: settings.enabled, settings, effective: settings };
    return OK("noesis_status", this.status);
  }

  private create(command: Json): Result {
    if (this.refuse !== undefined) return ERR("invalid_argument", this.refuse);
    this.seq += 1;
    const id = `ns_faux_${this.seq}`;
    const at = stamp(this.seq);
    const session: aether.NoesisSession = { id, agent: `agt_noesis_${this.seq}`, goal: String(command.goal), mode: command.mode === "practice" ? "practice" : "teach", phase: "probe", created_at: at, updated_at: at };
    this.addSession(session);
    this.event(session.agent, { type: "noesis.session_created", session });
    return OK("noesis_session", session);
  }

  /** Records the answer and, as the teaching turn would, reveals the result with its key. */
  private answer(command: Json): Result {
    const session = this.session(String(command.session));
    const held = this.held.get(String(command.question));
    if (held === undefined || session.open_question?.id !== held.question.id) return ERR("conflict", "that question is not open");
    const given = command.answer as { choices?: string[]; dont_know: boolean };
    const correct = held.question.kind === "goal" ? null : !given.dont_know && sameChoices(given.choices ?? [], held.key);
    const result: aether.QuestionResult = { question: held.question.id, answer_key: held.key, correct, explanation: held.explanation };
    this.update(session.id, { open_question: null });
    queueMicrotask(() => this.event(session.agent, { type: "noesis.answer_recorded", session: session.id, result }));
    return OK("ok", null);
  }

  private graphSlice(query: unknown): { nodes: aether.Proposition[]; chains: aether.Chain[] } {
    if (typeof query !== "string" || query === "") return this.graph;
    const words = query.toLowerCase();
    const nodes = this.graph.nodes.filter((node) => `${node.name} ${node.statement}`.toLowerCase().includes(words));
    return { nodes, chains: this.graph.chains };
  }

  private deleteProposition(id: string): Result {
    const before = this.graph.nodes.length;
    this.graph = { nodes: this.graph.nodes.filter((node) => node.id !== id), chains: this.graph.chains.filter((chain) => chain.conclusion !== id && !chain.premises.includes(id)) };
    return before === this.graph.nodes.length ? ERR("not_found", `proposition \`${id}\``) : OK("ok", null);
  }
}

function nestedPatch(flat: NoesisSettingsPatch): aether.NoesisSettingsPatch {
  const nested: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(flat)) {
    const [root, group, leaf] = key.split(".");
    invariant(root === "noesis" && group !== undefined, `${key} is a Noesis key`);
    if (leaf === undefined) nested[group] = value;
    else nested[group] = { ...(nested[group] as Json | undefined), [leaf]: value };
  }
  return nested as aether.NoesisSettingsPatch;
}

function mergeSettings(settings: aether.NoesisSettingsView, patch: aether.NoesisSettingsPatch): aether.NoesisSettingsView {
  return {
    ...settings,
    enabled: patch.enabled ?? settings.enabled,
    billing: { allow_api: patch.billing?.allow_api ?? settings.billing.allow_api },
    budget: { ...settings.budget, ...definedOf(patch.budget) },
    models: { ...settings.models, ...definedOf(patch.models) },
    extraction: { ...settings.extraction, ...definedOf(patch.extraction) },
  };
}

function sameChoices(given: readonly string[], key: aether.AnswerKey): boolean {
  const expected = typeof key === "string" ? [key] : key;
  return given.length === expected.length && [...given].sort().join("\n") === [...expected].sort().join("\n");
}

function definedOf<T extends object>(value: T | null | undefined): { [K in keyof T]?: NonNullable<T[K]> } {
  return Object.fromEntries(Object.entries(value ?? {}).filter(([, v]) => v !== null && v !== undefined)) as { [K in keyof T]?: NonNullable<T[K]> };
}

function noesisAgent(id: string): aether.Agent {
  return {
    id, name: "noesis-teacher", role: "noesis", run: "run_01JFAUXNOESIS00000000000001", model: { provider: "anthropic", model: "faux" },
    config: {}, scope: {}, status: { state: "idle" }, created_at: stamp(0), updated_at: stamp(0), last_event_at: stamp(0),
    stats: { turns: 0, tool_calls: 0, context_tokens: 0, usage: { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 0, cost_complete: true } },
  } as unknown as aether.Agent;
}

export const stamp = (n: number): string => new Date(Date.UTC(2026, 10, 4, 9, 0, 0) + n * 7_000).toISOString();
