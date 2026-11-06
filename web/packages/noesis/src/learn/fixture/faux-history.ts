// What the faux daemon remembers about a session beyond its row, as `noesis_get_session` returns it:
// the agents with the inputs Noesis wrote to them, every question and problem with its outcome, usage.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "../model.ts";

const ZERO: aether.Usage = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 0, cost_complete: true };

interface Held {
  agents: aether.SessionAgent[];
  questions: aether.AskedQuestion[];
  problems: aether.PosedProblem[];
  usage: aether.SessionUsage;
  context: aether.EvidenceBundle | undefined;
}

export class SessionHistory {
  private readonly held = new Map<string, Held>();

  start(session: aether.NoesisSession, context?: aether.EvidenceBundle): void {
    invariant(!this.held.has(session.id), `session ${session.id} starts once`);
    const role: aether.SessionRole = session.mode === "practice" ? "practitioner" : session.phase === "probe" ? "prober" : "teacher";
    this.held.set(session.id, { agents: [{ agent: session.agent, role }], questions: [], problems: [], usage: { subscription: ZERO, api: ZERO }, context });
  }

  /** The session as `noesis_get_session` reads it: the row with its whole history. */
  read(session: aether.NoesisSession): aether.NoesisSession {
    const held = this.of(session.id);
    return { ...session, agents: held.agents, questions: held.questions, problems: held.problems, usage: held.usage, ...(held.context ? { context: held.context } : {}) };
  }

  handOff(session: string, agent: string, role: aether.SessionRole): void {
    const held = this.of(session);
    invariant(!held.agents.some((entry) => entry.agent === agent), "a hand-off names a new agent");
    held.agents.push({ agent, role });
  }

  /** Marks transcript entry `seq` of `agent` as Noesis's own input, never the learner's words. */
  internal(session: string, agent: string, seq: number): void {
    const entry = this.of(session).agents.find((candidate) => candidate.agent === agent);
    invariant(entry !== undefined, `agent ${agent} ran session ${session}`);
    entry.internal = [...(entry.internal ?? []), seq];
  }

  posed(session: string, question: aether.Question): void {
    this.of(session).questions.push({ question });
  }

  answered(session: string, answer: aether.RecordedAnswer, result: aether.QuestionResult): void {
    const asked = this.of(session).questions.find((entry) => entry.question.id === result.question);
    invariant(asked !== undefined && asked.result === undefined, "an answer closes an open question once");
    asked.answer = answer;
    asked.result = result;
  }

  problem(session: string, problem: aether.Problem): void {
    this.of(session).problems.push({ problem });
  }

  concluded(session: string, problem: string, conclusion: aether.ProblemConclusion): void {
    const posed = this.of(session).problems.find((entry) => entry.problem.id === problem);
    invariant(posed !== undefined && posed.conclusion === undefined, "a problem concludes once");
    posed.conclusion = conclusion;
  }

  bill(session: string, usage: aether.SessionUsage): void {
    this.of(session).usage = usage;
  }

  forget(session: string): void {
    this.held.delete(session);
  }

  private of(session: string): Held {
    const held = this.held.get(session);
    invariant(held !== undefined, `the faux daemon has the history of ${session}`);
    return held;
  }
}
