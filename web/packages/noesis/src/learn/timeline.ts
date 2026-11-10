// The lesson's chat in time order: every agent's transcript prose (minus what Noesis wrote to them),
// the questions with the learner's answers and results, the problems with their conclusions, and the
// current agent's text still streaming. The session read is the record; events only arrive sooner.

import { compareStamps, type NoesisCache, type StreamState } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { invariant, messageText } from "./model.ts";

/** How a problem ended, from the session read or the `noesis.practice_concluded` event. */
export type Conclusion = Pick<aether.ProblemConclusion, "outcome" | "rung" | "miss_class" | "retest" | "solution">;

export type TimelineItem =
  | { readonly kind: "message"; readonly key: string; readonly at: string; readonly role: "learner" | "teacher"; readonly text: string; readonly live: boolean }
  | {
    readonly kind: "question"; readonly key: string; readonly at: string; readonly question: aether.Question;
    readonly answer: aether.RecordedAnswer | undefined; readonly result: aether.QuestionResult | undefined; readonly open: boolean;
  }
  | { readonly kind: "problem"; readonly key: string; readonly at: string; readonly problem: aether.Problem; readonly conclusion: Conclusion | undefined; readonly active: boolean };

/** One agent's transcript as the lesson shows it. */
export interface AgentTranscript {
  readonly agent: aether.SessionAgent;
  readonly entries: readonly aether.TranscriptEntry[];
}

/** The agents that ran the session, oldest first; a read without history still names the current one. */
export function sessionAgents(session: aether.NoesisSession): readonly aether.SessionAgent[] {
  const agents = session.agents ?? [];
  invariant(new Set(agents.map((entry) => entry.agent)).size === agents.length, "a session names each agent once");
  if (agents.some((entry) => entry.agent === session.agent)) return agents;
  return [...agents, { agent: session.agent, role: session.mode === "practice" ? "practitioner" : "teacher" }];
}

/**
 * The seqs Noesis wrote to an agent. A session from before the history was kept names none; its
 * agent's first input, before any reply, is still Noesis's own (the opening or the hand-off).
 */
export function internalSeqs(agent: aether.SessionAgent, entries: readonly aether.TranscriptEntry[]): ReadonlySet<number> {
  if (agent.internal !== undefined && agent.internal.length > 0) return new Set(agent.internal);
  const first = entries.reduce<aether.TranscriptEntry | undefined>((low, entry) => (low === undefined || entry.seq < low.seq ? entry : low), undefined);
  return first?.message.role === "user" ? new Set([first.seq]) : new Set();
}

function messageItems({ agent, entries }: AgentTranscript): TimelineItem[] {
  const internal = internalSeqs(agent, entries);
  const items: TimelineItem[] = [];
  for (const entry of entries) {
    const role = entry.message.role === "assistant" ? "teacher" : entry.message.role === "user" ? "learner" : null;
    // Noesis's own inputs (the opening context, a hand-off) are user-role but never the learner's words.
    if (role === null || (role === "learner" && internal.has(entry.seq))) continue;
    const content = role === "learner"
      ? entry.message.content.filter((block) => block.type !== "text" || !isAnswerEnvelope(block.text))
      : entry.message.content;
    const text = messageText({ ...entry.message, content });
    if (text !== "") items.push({ kind: "message", key: `m${agent.agent}:${entry.seq}`, at: entry.at, role, text, live: false });
  }
  return items;
}

function isAnswerEnvelope(text: string): boolean {
  const prefix = "The learner answered: ";
  if (!text.startsWith(prefix)) return false;
  let value: unknown;
  try {
    value = JSON.parse(text.slice(prefix.length));
  } catch {
    return false;
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const answer = value as Record<string, unknown>;
  const allowed = new Set(["choices", "chosen", "text", "dont_know", "confidence", "thinking", "question", "correct", "answer_key"]);
  if (Object.keys(answer).some((key) => !allowed.has(key)) || typeof answer.dont_know !== "boolean") return false;
  for (const key of ["choices", "chosen"]) {
    const field = answer[key];
    if (field !== undefined && (!Array.isArray(field) || !field.every((item: unknown) => typeof item === "string"))) return false;
  }
  for (const key of ["text", "thinking"]) {
    if (answer[key] != null && typeof answer[key] !== "string") return false;
  }
  if (answer.question !== undefined && (typeof answer.question !== "string" || answer.question === "")) return false;
  if (answer.correct !== undefined && typeof answer.correct !== "boolean") return false;
  if (answer.answer_key !== undefined && typeof answer.answer_key !== "string") return false;
  if (answer.confidence != null && (typeof answer.confidence !== "number" || !Number.isInteger(answer.confidence) || answer.confidence < 0 || answer.confidence > 100)) return false;
  const hasChoice = (Array.isArray(answer.choices) && answer.choices.length > 0) || (Array.isArray(answer.chosen) && answer.chosen.length > 0);
  const hasText = typeof answer.text === "string" && answer.text.trim() !== "";
  return answer.dont_know ? !hasChoice && answer.text == null && answer.confidence == null : hasChoice || hasText;
}

function questionItems(session: aether.NoesisSession, learn: NoesisCache): TimelineItem[] {
  const asked = new Map((session.questions ?? []).map((entry) => [entry.question.id, entry]));
  for (const { session: owner, question } of Object.values(learn.questions)) {
    if (owner === session.id && !asked.has(question.id)) asked.set(question.id, { question });
  }
  return [...asked.values()].map(({ question, answer, result }) => {
    const eventResult = learn.results[question.id];
    const held = result?.correct == null && eventResult?.correct != null ? eventResult : result ?? eventResult;
    invariant(held === undefined || held.question === question.id, "a result belongs to its question");
    return { kind: "question", key: question.id, at: question.posed_at, question, answer: answer ?? undefined, result: held, open: session.open_question?.id === question.id };
  });
}

function problemItems(session: aether.NoesisSession, learn: NoesisCache): TimelineItem[] {
  const posed = new Map((session.problems ?? []).map((entry) => [entry.problem.id, entry]));
  for (const { session: owner, problem } of Object.values(learn.problems)) {
    if (owner === session.id && !posed.has(problem.id)) posed.set(problem.id, { problem });
  }
  return [...posed.values()].map(({ problem, conclusion }) => ({
    kind: "problem", key: problem.id, at: problem.posed_at, problem,
    conclusion: conclusion ?? learn.conclusions[problem.id], active: session.active_problem?.id === problem.id,
  }));
}

/** The current agent's uncommitted text, oldest invocation first: the committed message replaces it. */
export function liveItems(streams: Readonly<Record<string, StreamState>>, agent: string): TimelineItem[] {
  const live = Object.values(streams).filter((stream) => stream.agent === agent && !stream.committed && stream.text.trim() !== "");
  live.sort((a, b) => a.invocation.localeCompare(b.invocation));
  return live.map((stream) => ({ kind: "message", key: `live:${stream.invocation}`, at: "", role: "teacher", text: stream.text.trim(), live: true }));
}

export function buildTimeline(session: aether.NoesisSession, transcripts: readonly AgentTranscript[], learn: NoesisCache): TimelineItem[] {
  const items = [...transcripts.flatMap(messageItems), ...questionItems(session, learn), ...problemItems(session, learn)];
  const order = new Map(items.map((item, index) => [item, index]));
  items.sort((a, b) => compareStamps(a.at, b.at) || (order.get(a) ?? 0) - (order.get(b) ?? 0));
  invariant(items.filter((item) => item.kind === "question" && item.open).length <= 1, "a session has at most one open question");
  invariant(new Set(items.map((item) => item.key)).size === items.length, "every timeline item has its own key");
  return items;
}
