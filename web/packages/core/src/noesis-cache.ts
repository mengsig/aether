// The Noesis entities one Aether connection holds (CONTRACTS.md §6.2): sessions, the reads the
// learning screens show, and what events reveal. Pure updates; core's store is their only home.

import type { aether } from "@aether-ui/protocol";
import { compareStamps } from "./stamps.ts";
import { array, invariant, object, omit, string, type Json } from "./guards.ts";

/** A whole reread reply, stamped with the ask that produced it: a later ask is at least as new. */
export interface NoesisRead {
  readonly value: Json;
  readonly ticket: number;
}

export interface NoesisQuestionEntry {
  readonly session: string;
  readonly question: aether.Question;
}

export interface NoesisProblemEntry {
  readonly session: string;
  readonly problem: aether.Problem;
}

export interface NoesisCache {
  readonly sessions: Readonly<Record<string, aether.NoesisSession>>;
  readonly sessionChanges: Readonly<Record<string, { readonly ticket: number; readonly present: boolean }>>;
  /** Sessions deleted from this client: a reply asked before the delete cannot bring one back. */
  readonly deleted: readonly string[];
  readonly reads: Readonly<Record<string, NoesisRead>>;
  /** Questions and problems as posed, kept after the session moves on, so answered ones stay shown. */
  readonly questions: Readonly<Record<string, NoesisQuestionEntry>>;
  readonly problems: Readonly<Record<string, NoesisProblemEntry>>;
  /** Reveals: `noesis.answer_recorded` results by question, `noesis.practice_concluded` by problem. */
  readonly results: Readonly<Record<string, aether.QuestionResult>>;
  readonly conclusions: Readonly<Record<string, aether.EventKindNoesisPracticeConcluded>>;
  /** Bumped when an event makes an entity stale; screens showing it reread on a change. */
  readonly versions: Readonly<Record<string, number>>;
  /** The last ask handed out; it only grows, so it orders reads across reconnects. */
  readonly ticket: number;
}

export const EMPTY_NOESIS: NoesisCache = Object.freeze({
  sessions: {}, sessionChanges: {}, deleted: [], reads: {}, questions: {}, problems: {}, results: {}, conclusions: {}, versions: {}, ticket: 0,
});

/** Keys of the stale versions and stamped reads. */
export const noesisKey = {
  session: (id: string): string => `session:${id}`,
  sessions: "sessions",
  "all-sessions": "all-sessions",
  graph: "graph",
  learner: "learner",
  jobs: "jobs",
  status: "status",
} as const;

const MAX_DELETED = 200;

/** Hands out the next ticket; the caller stamps the read it is about to send with it. */
export function nextTicket(cache: NoesisCache): NoesisCache {
  return { ...cache, ticket: cache.ticket + 1 };
}

/** Installs a reread reply whole, unless a read asked later is already held. */
export function installRead(cache: NoesisCache, key: string, value: Json, ticket: number): NoesisCache {
  invariant(Number.isSafeInteger(ticket) && ticket > 0 && ticket <= cache.ticket, "a read is stamped with a ticket handed out");
  const held = cache.reads[key];
  if (held && held.ticket > ticket) return cache;
  return { ...cache, reads: { ...cache.reads, [key]: { value, ticket } } };
}

/** Installs a session whole when it is at least as new as the one held; a deleted one stays gone. */
export function installSession(cache: NoesisCache, session: aether.NoesisSession): NoesisCache {
  return installSessionRows(cache, [session]);
}

function installSessionRows(cache: NoesisCache, rows: readonly aether.NoesisSession[]): NoesisCache {
  const sessions = Object.assign(Object.create(null) as Record<string, aether.NoesisSession>, cache.sessions);
  let questions: Record<string, NoesisQuestionEntry> | undefined;
  let problems: Record<string, NoesisProblemEntry> | undefined;
  const deleted = new Set(cache.deleted);
  let changed = false;
  for (const session of rows) {
    const id = string(session.id, "noesis_session.id");
    string(session.updated_at, "noesis_session.updated_at");
    if (deleted.has(id)) continue;
    const held = sessions[id];
    if (held && compareStamps(held.updated_at, session.updated_at) > 0) continue;
    sessions[id] = withHeldHistory(held, session);
    if (session.open_question) {
      const question = session.open_question;
      const key = string(question.id, "question.id");
      if ((questions ?? cache.questions)[key]?.question !== question) {
        questions ??= Object.assign(Object.create(null) as Record<string, NoesisQuestionEntry>, cache.questions);
        questions[key] = { session: id, question };
      }
    }
    if (session.active_problem) {
      const problem = session.active_problem;
      const key = string(problem.id, "problem.id");
      if ((problems ?? cache.problems)[key]?.problem !== problem) {
        problems ??= Object.assign(Object.create(null) as Record<string, NoesisProblemEntry>, cache.problems);
        problems[key] = { session: id, problem };
      }
    }
    changed = true;
  }
  return changed ? { ...cache, sessions, questions: questions ?? cache.questions, problems: problems ?? cache.problems } : cache;
}

const HISTORY = ["agents", "questions", "problems", "usage", "context"] as const;

/**
 * Only `noesis_get_session` carries a session's history, and it always names the session's agents.
 * A list row or an event without it keeps the history held, which the session's reread refreshes.
 */
function withHeldHistory(held: aether.NoesisSession | undefined, session: aether.NoesisSession): aether.NoesisSession {
  if (held?.agents === undefined || session.agents !== undefined) return session;
  invariant(held.id === session.id, "history is kept for its own session");
  const kept = Object.fromEntries(HISTORY.filter((key) => session[key] === undefined && held[key] !== undefined).map((key) => [key, held[key]]));
  return { ...session, ...kept };
}

/** Installs a page of `noesis_list_sessions`: each row whole by its own stamp, and the page as a read. */
export function installSessionList(cache: NoesisCache, page: Json, ticket: number): NoesisCache {
  const rows = array(page.sessions, "noesis_sessions.sessions").map((row) => object(row, "noesis session") as unknown as aether.NoesisSession);
  const installed = installSessionRows(cache, rows);
  return installRead(installed, noesisKey.sessions, page, ticket);
}


/** Installs one completed list, retaining membership changes made after it was requested. */
export function installAllSessions(cache: NoesisCache, page: Json, ticket: number): NoesisCache {
  const held = cache.reads[noesisKey["all-sessions"]];
  if (held && held.ticket > ticket) return cache;
  const rows = array(page.sessions, "noesis_sessions.sessions").map((row) => object(row, "noesis session") as unknown as aether.NoesisSession);
  const ids = new Set(rows.map((row) => string(row.id, "noesis session.id")));
  for (const [id, change] of Object.entries(cache.sessionChanges)) {
    if (change.ticket <= ticket) continue;
    if (change.present) ids.add(id); else ids.delete(id);
  }
  const installed = installSessionRows(cache, rows);
  for (const id of installed.deleted) ids.delete(id);
  return installRead(installed, noesisKey["all-sessions"], { ids: [...ids] }, ticket);
}

/** Records membership changes so an in-flight complete list cannot undo them. */
export function sessionMembership(cache: NoesisCache, id: string, present: boolean): NoesisCache {
  string(id, "session id");
  const ticket = cache.ticket + 1;
  invariant(Number.isSafeInteger(ticket), "session change ticket is a safe integer");
  const read = cache.reads[noesisKey["all-sessions"]];
  let reads = cache.reads;
  if (read) {
    const ids = new Set(array(read.value.ids, "all-sessions.ids").map((value) => string(value, "session id")));
    if (present) ids.add(id); else ids.delete(id);
    reads = { ...reads, [noesisKey["all-sessions"]]: { ...read, value: { ids: [...ids] } } };
  }
  return { ...cache, ticket, reads, sessionChanges: { ...cache.sessionChanges, [id]: { ticket, present } } };
}

export function removeSession(cache: NoesisCache, id: string): NoesisCache {
  string(id, "session id");
  const deleted = [...cache.deleted.filter((held) => held !== id), id].slice(-MAX_DELETED);
  return sessionMembership({ ...cache, sessions: omit(cache.sessions, id), deleted, versions: bump(cache.versions, noesisKey.sessions) }, id, false);
}

function withQuestion(cache: NoesisCache, session: string, question: aether.Question): NoesisCache {
  string(question.id, "question.id");
  if (cache.questions[question.id]?.question === question) return cache;
  return { ...cache, questions: { ...cache.questions, [question.id]: { session, question } } };
}

function withProblem(cache: NoesisCache, session: string, problem: aether.Problem): NoesisCache {
  string(problem.id, "problem.id");
  if (cache.problems[problem.id]?.problem === problem) return cache;
  return { ...cache, problems: { ...cache.problems, [problem.id]: { session, problem } } };
}

function bump(versions: Readonly<Record<string, number>>, ...keys: readonly string[]): Readonly<Record<string, number>> {
  const next = { ...versions };
  for (const key of keys) next[key] = (next[key] ?? 0) + 1;
  return next;
}

/** Applies one durable event: reveals are kept, posed objects recorded, the entities it changes marked stale. */
export function applyNoesisEvent(cache: NoesisCache, kind: Json, agent: string | undefined): NoesisCache {
  const type = string(kind.type, "event.kind.type");
  if (type === "agent.message") return agent === undefined ? cache : staleByAgent(cache, agent);
  if (!type.startsWith("noesis.")) return cache;
  const session = typeof kind.session === "string" ? kind.session : null;
  let next = cache;
  if (type === "noesis.session_created") {
    const created = object(kind.session, "noesis.session_created.session") as unknown as aether.NoesisSession;
    next = sessionMembership({ ...installSession(next, created), versions: bump(next.versions, noesisKey.sessions) }, created.id, true);
  } else if (type === "noesis.question_posed" && session) {
    next = withQuestion(next, session, object(kind.question, "noesis.question_posed.question") as unknown as aether.Question);
  } else if (type === "noesis.problem_posed" && session) {
    next = withProblem(next, session, object(kind.problem, "noesis.problem_posed.problem") as unknown as aether.Problem);
  } else if (type === "noesis.answer_recorded") {
    const result = object(kind.result, "noesis.answer_recorded.result") as unknown as aether.QuestionResult;
    next = { ...next, results: { ...next.results, [string(result.question, "result.question")]: result } };
  } else if (type === "noesis.practice_concluded") {
    const concluded = kind as unknown as aether.EventKindNoesisPracticeConcluded;
    next = { ...next, conclusions: { ...next.conclusions, [string(concluded.problem, "concluded.problem")]: concluded } };
  } else if (type === "noesis.settings_changed") {
    next = stampStatus(next);
  }
  const stale = staleKeys(type, session, kind);
  return stale.length === 0 ? next : { ...next, versions: bump(next.versions, ...stale) };
}

/** Which held entities an event makes stale; a job that ends has merged what it found into the graph. */
function staleKeys(type: string, session: string | null, kind: Json): string[] {
  if (type === "noesis.learner_updated") return [noesisKey.learner];
  if (type === "noesis.graph_merged") return [noesisKey.graph];
  if (type === "noesis.job_updated") return jobEnded(kind) ? [noesisKey.jobs, noesisKey.graph] : [noesisKey.jobs];
  if (type === "noesis.settings_changed") return [noesisKey.status];
  if (session === null) return [];
  return [noesisKey.session(session), noesisKey.sessions];
}

const ENDED_JOB = new Set<string>(["succeeded", "failed", "cancelled"]);

function jobEnded(kind: Json): boolean {
  const job = object(kind.job, "noesis.job_updated.job");
  return ENDED_JOB.has(string(job.state, "noesis.job_updated.job.state"));
}

/** A settings event is newer than any status ask still in flight: its read takes a fresh ticket. */
function stampStatus(cache: NoesisCache): NoesisCache {
  const ticket = cache.ticket + 1;
  invariant(Number.isSafeInteger(ticket), "the ticket counter stays a safe integer");
  return { ...cache, ticket, reads: { ...cache.reads, [noesisKey.status]: { value: {}, ticket } } };
}

/** A committed message of a session's agent: its turn moved, so the session is reread. */
function staleByAgent(cache: NoesisCache, agent: string): NoesisCache {
  const owners = Object.values(cache.sessions).filter((session) => session.agent === agent).map((session) => noesisKey.session(session.id));
  return owners.length === 0 ? cache : { ...cache, versions: bump(cache.versions, ...owners) };
}
