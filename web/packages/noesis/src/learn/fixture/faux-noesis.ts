// A faux aetherd that serves Noesis, behind an in-memory Transport, for the learning-screen tests and
// the screenshot page. Answer keys stay here until an answer is recorded, as the daemon seals them.

import type { aether, NoesisSettingsPatch, PrivilegedAction, Transport } from "@aether-ui/protocol";
import { invariant } from "../model.ts";
import { SessionHistory } from "./faux-history.ts";

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
  /** The evidence each session started from, by session id. */
  readonly contexts?: Readonly<Record<string, aether.EvidenceBundle>>;
  /** What `list_models` and `providers` answer; none by default. */
  readonly models?: readonly aether.ModelInfo[];
  readonly providers?: readonly aether.ProviderInfo[];
  readonly retryTurn?: boolean;
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
  /** The message `noesis_send` and `noesis_answer` refuse with, as at a spent budget. */
  refuseTurns: string | undefined;
  /** Off, an answer is recorded without its `noesis.answer_recorded` event: only a reread shows it. */
  answerEvents = true;
  readonly history = new SessionHistory();
  jobs: aether.Job[] = [];
  models: readonly aether.ModelInfo[];
  providers: readonly aether.ProviderInfo[];
  readonly retryTurn: boolean;
  readonly sessions = new Map<string, aether.NoesisSession>();
  readonly held = new Map<string, HeldQuestion>();
  graph: { nodes: aether.Proposition[]; chains: aether.Chain[] };
  concepts: aether.ConceptState[];
  private readonly agents = new Map<string, aether.Agent>();
  private readonly transcripts = new Map<string, aether.TranscriptEntry[]>();
  private durableSub: number | null = null;
  private readonly deltaSubs = new Map<string, number>();
  private nextSub = 1;
  private seq = 0;
  private link: Link | null = null;

  constructor(options: FauxNoesisOptions) {
    this.status = options.status;
    this.refuse = options.refuse;
    this.models = options.models ?? [];
    this.providers = options.providers ?? [];
    this.retryTurn = options.retryTurn ?? false;
    this.graph = { nodes: [...(options.graph?.nodes ?? [])], chains: [...(options.graph?.chains ?? [])] };
    this.concepts = [...(options.concepts ?? [])];
    for (const session of options.sessions ?? []) this.addSession(session, options.contexts?.[session.id]);
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
  teach(session: string, text: string, invocation: string | null = null): void {
    const agent = this.session(session).agent;
    const entry = this.append(agent, { role: "assistant", content: [{ type: "text", text }] }, invocation);
    this.event(agent, { type: "agent.message", role: "assistant", content: entry.message.content, invocation });
  }

  /** The teacher's text while it is written: a delta on the agent's delta subscription, if one is open. */
  stream(session: string, invocation: string, order: number, text: string): void {
    const agent = this.session(session).agent;
    const sub = this.deltaSubs.get(agent);
    invariant(sub !== undefined, `a screen watches ${agent} for deltas`);
    this.push({ frame: "delta", sub, agent, invocation, invocation_seq: order, delta: { delta: "text", text } });
  }

  /** Noesis writes to the session's agent: a user-role message that is never the learner's. */
  internal(session: string, text: string): void {
    const agent = this.session(session).agent;
    const entry = this.append(agent, { role: "user", content: [{ type: "text", text }] });
    this.history.internal(session, agent, entry.seq);
  }

  /** The session moves to a new agent, as the Prober hands over to the Teacher. */
  handOff(session: string, phase: aether.Phase, role: aether.SessionRole): string {
    this.seq += 1;
    const agent = `agt_noesis_${role}_${this.seq}`;
    this.agents.set(agent, noesisAgent(agent));
    this.history.handOff(session, agent, role);
    this.update(session, { agent, phase });
    this.event(agent, { type: "noesis.phase_changed", session, phase });
    return agent;
  }

  /** Poses a question: the session's open question, a `noesis.question_posed` event, and its key held back. */
  pose(session: string, held: HeldQuestion): void {
    const question = { ...held.question, posed_at: stamp(this.seq + 1) };
    this.held.set(question.id, { ...held, question });
    this.history.posed(session, question);
    this.update(session, { open_question: question });
    this.event(this.session(session).agent, { type: "noesis.question_posed", session, question });
  }

  /** Stops the session's turn on `stall`, as the daemon does before a model call: the session carries it and an event says so. */
  stall(session: string, stall: aether.SessionStall): void {
    this.update(session, { stalled: stall });
    this.event(this.session(session).agent, { type: "noesis.session_stalled", session, stall });
  }

  problem(session: string, posed: aether.Problem): void {
    const problem = { ...posed, posed_at: stamp(this.seq + 1) };
    this.history.problem(session, problem);
    this.update(session, { active_problem: problem, phase: "practice" });
    this.event(this.session(session).agent, { type: "noesis.problem_posed", session, problem });
  }

  conclude(session: string, conclusion: Omit<aether.EventKindNoesisPracticeConcluded, "type" | "session">): void {
    const { problem, ...ending } = conclusion;
    this.history.concluded(session, problem, { ...ending, concluded_at: stamp(this.seq + 1) });
    this.update(session, { active_problem: null });
    this.event(this.session(session).agent, { type: "noesis.practice_concluded", session, ...conclusion });
  }

  /** A question asked and answered before this page ever ran: only the session read knows of it. */
  answeredEarlier(session: string, held: HeldQuestion, given: { choices?: string[]; text?: string; dont_know: boolean; confidence?: number }): void {
    this.seq += 1;
    const question = { ...held.question, posed_at: stamp(this.seq) };
    const correct = question.kind === "goal" ? null : !given.dont_know && sameChoices(given.choices ?? [], held.key);
    this.history.posed(session, question);
    this.history.answered(session, recorded(question, given, stamp(this.seq + 1)), { question: question.id, answer_key: held.key, correct, explanation: held.explanation });
    this.seq += 1;
  }

  /** Moves a job to `state`, announcing it unless `quiet`, as when the event never reached the page. */
  finishJob(id: string, state: aether.JobState, quiet = false): void {
    const job = this.jobs.find((candidate) => candidate.id === id);
    invariant(job !== undefined, `the faux daemon has job ${id}`);
    this.seq += 1;
    const next = { ...job, state, updated_at: stamp(this.seq) };
    this.jobs = this.jobs.map((candidate) => (candidate.id === id ? next : candidate));
    if (!quiet) this.event(null, { type: "noesis.job_updated", job: next });
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

  private addSession(session: aether.NoesisSession, context?: aether.EvidenceBundle): void {
    invariant(!this.sessions.has(session.id), `session ${session.id} is new`);
    this.sessions.set(session.id, session);
    this.history.start(session, context);
    this.agents.set(session.agent, noesisAgent(session.agent));
    if (session.open_question) this.held.set(session.open_question.id, { question: session.open_question, key: [], explanation: "" });
  }

  private update(id: string, patch: Partial<aether.NoesisSession>): void {
    this.seq += 1;
    this.sessions.set(id, { ...this.session(id), ...patch, updated_at: stamp(this.seq) });
  }

  private append(agent: string, message: aether.Message, invocation: string | null = null): aether.TranscriptEntry {
    this.seq += 1;
    const entry: aether.TranscriptEntry = { seq: this.seq, at: stamp(this.seq), invocation, message };
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
        features: ["deltas", "delta_order", "transcript_paging", "agents_paging", "noesis", ...(this.retryTurn ? ["noesis_retry_turn"] : [])], device: { id: "dev_faux", name: "faux", role: "operator" }, idle_timeout_secs: null,
      });
      case "subscribe": return this.subscribe(command);
      case "unsubscribe":
        for (const [agent, sub] of this.deltaSubs) if (sub === command.id) this.deltaSubs.delete(agent);
        return OK("ok", null);
      case "get_attention": return OK("attention", { agents: [], approvals: [], questions: [], last_seq: this.seq });
      case "list_agents_page": return OK("agents_page", { agents: [...this.agents.values()] });
      case "get_agent": return this.agents.has(String(command.id)) ? OK("agent", this.agents.get(String(command.id))) : ERR("not_found", "agent");
      case "get_transcript_page": return OK("transcript_page", this.page(command));
      case "list_models": return OK("models", this.models);
      case "providers": return OK("providers", this.providers);
      default: return ERR("invalid_argument", `the faux daemon has no ${cmd}`);
    }
  }

  private subscribe(command: Json): Result {
    const id = this.nextSub++;
    if (command.deltas !== true) this.durableSub = id;
    else this.deltaSubs.set(String((command.query as Json).agent), id);
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
      case "noesis_list_sessions": return OK("noesis_sessions", { sessions: [...this.sessions.values()].slice(0, typeof command.limit === "number" ? command.limit : undefined) });
      case "noesis_get_session": {
        const session = this.sessions.get(String(command.session));
        return session ? OK("noesis_session", this.history.read(session)) : ERR("not_found", `session \`${String(command.session)}\``);
      }
      case "noesis_create_session": return this.create(command);
      case "noesis_send": return this.refuseTurns === undefined ? OK("ok", null) : ERR("invalid_argument", this.refuseTurns);
      case "noesis_retry_turn": return this.retryTurn ? this.retry(command) : ERR("invalid_argument", "unsupported");
      case "noesis_answer": return this.refuseTurns === undefined ? this.answer(command) : ERR("invalid_argument", this.refuseTurns);
      case "noesis_delete_session": this.sessions.delete(String(command.session)); this.history.forget(String(command.session)); return OK("ok", null);
      case "noesis_extract": return this.extract(String(command.session));
      case "noesis_jobs": return OK("noesis_jobs", { jobs: this.jobs });
      case "noesis_graph": return OK("noesis_graph", { ...this.graphSlice(command.query), clusters: [], truncated: false });
      case "noesis_learner": return OK("noesis_learner", { concepts: this.concepts });
      case "noesis_delete_proposition": return this.deleteProposition(String(command.proposition));
      case "noesis_delete_source": return OK("ok", null);
      default: return ERR("invalid_argument", `the faux daemon has no ${cmd}`);
    }
  }

  private retry(command: Json): Result {
    const session = this.session(String(command.session));
    invariant(this.retryTurn, "retry is advertised");
    if (this.refuseTurns !== undefined) return ERR("invalid_argument", this.refuseTurns);
    this.update(session.id, { stalled: null });
    return OK("ok", null);
  }

  /** Holds every privileged action until the returned function is called, to finish a save late. */
  holdPrivileged(): () => void {
    let open = () => {};
    this.gate = new Promise<void>((resolve) => { open = resolve; });
    return open;
  }

  /** Settles once held privileged actions may run; the bridge awaits it before each action. */
  gate: Promise<void> = Promise.resolve();

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
    const given = command.answer as { choices?: string[]; text?: string; dont_know: boolean; confidence?: number };
    const correct = held.question.kind === "goal" ? null : !given.dont_know && sameChoices(given.choices ?? [], held.key);
    const result: aether.QuestionResult = { question: held.question.id, answer_key: held.key, correct, explanation: held.explanation };
    this.update(session.id, { open_question: null });
    this.history.answered(session.id, recorded(held.question, given, stamp(this.seq)), result);
    if (this.answerEvents) queueMicrotask(() => this.event(null, { type: "noesis.answer_recorded", session: session.id, result }));
    return OK("ok", null);
  }

  private extract(session: string): Result {
    this.seq += 1;
    const at = stamp(this.seq);
    const job: aether.Job = { id: `job_faux_${this.seq}`, kind: "extract_session", state: "queued", target: session, created_at: at, updated_at: at, usage: ZERO_USAGE };
    this.jobs = [...this.jobs, job];
    return OK("noesis_job", { job });
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
    engineering_context: patch.engineering_context ?? settings.engineering_context,
  };
}

const ZERO_USAGE: aether.Usage = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 0, cost_complete: true };

/** The answer as the daemon records it: the chosen options by label, in the order they were shown. */
function recorded(question: aether.Question, given: { choices?: string[]; text?: string; dont_know: boolean; confidence?: number }, at: string): aether.RecordedAnswer {
  const chosen = (question.options ?? []).filter((option) => given.choices?.includes(option.value)).map((option) => option.label);
  return { chosen, dont_know: given.dont_know, answered_at: at, ...(given.text ? { text: given.text } : {}), ...(given.confidence === undefined ? {} : { confidence: given.confidence }) };
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
