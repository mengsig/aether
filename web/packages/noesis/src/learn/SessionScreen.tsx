// `#/learn/sessions/:id`: the lesson's whole conversation (every agent's, with its questions and
// problems), the plan beside it and the way to answer. The teacher's reply streams in as it is written.

import type { aether, RouteProps } from "@aether-ui/protocol";
import { Button, EmptyState, ErrorState } from "@aether-ui/ui";
import { useMemo } from "react";
import { useLearn, useNoesisRead, useOperateRefusal } from "./data.ts";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { Composer } from "./Composer.tsx";
import { CompletionCard, SourcesCard, UsageNote } from "./LessonCards.tsx";
import { useLessonTurn } from "./lesson-turn.ts";
import { LessonText } from "./LessonText.tsx";
import { exhaustedBudget, invariant, PHASE_LABEL, planComplete, type BudgetUse, type Refusal } from "./model.ts";
import { PlanTree } from "./PlanTree.tsx";
import { ProblemCard } from "./ProblemCard.tsx";
import { QuestionCard, type AnswerInput } from "./QuestionCard.tsx";
import { RefusalNote } from "./RefusalNote.tsx";
import { SessionActions } from "./SessionActions.tsx";
import { useLessonTranscripts, useLiveText } from "./session-data.ts";
import { buildTimeline, type TimelineItem } from "./timeline.ts";

export { STALL_MS } from "./lesson-turn.ts";

const BUDGET_REFUSAL: Refusal = { kind: "budget", message: "noesis.budget.interactive_usd_daily is spent for today" };

export function SessionScreen({ conn, status, id }: OpenGate & { id: string }) {
  const read = useNoesisRead(conn, { kind: "session", id });
  const learn = useLearn(conn);
  const session = learn.sessions[id];
  if (session === undefined) {
    if (learn.deleted.includes(id)) return <EmptyState title="Session deleted" action={<a className="ui-link" href="#/learn">All sessions</a>}>Its conversation stays in Aether history.</EmptyState>;
    if (read.error !== null) return <ErrorState title="Could not open this session" message={read.error} onRetry={read.reload} />;
    return <p className="learn-muted" aria-busy="true">Opening the session…</p>;
  }
  return <SessionView conn={conn} status={status} session={session} reload={read.reload} />;
}

function SessionView({ conn, status, session, reload }: OpenGate & { session: aether.NoesisSession; reload: () => void }) {
  const learn = useLearn(conn);
  const operate = useOperateRefusal(conn);
  const statusRead = useNoesisRead(conn, { kind: "status" });
  const transcripts = useLessonTranscripts(conn, session);
  const live = useLiveText(conn, session.agent);
  const items = useMemo(() => buildTimeline(session, transcripts, learn), [session, transcripts, learn]);
  const streamed = live.map((item) => (item.kind === "message" ? item.text : "")).join("");
  const turn = useLessonTurn(conn, session, items, streamed);
  const budget = exhaustedBudget(status);
  const refusal = turn.refusal ?? (budget === null ? null : BUDGET_REFUSAL);
  const answer = (question: string, input: AnswerInput) => turn.act({ cmd: "noesis_answer", session: session.id, question, answer: input });
  const checkAgain = () => {
    reload();
    turn.unstall();
  };
  return (
    <div className="learn-session">
      <div className="learn-chat">
        <h2 className="learn-session-goal">{session.goal}</h2>
        <p className="learn-muted">{PHASE_LABEL[session.phase]}</p>
        {session.context && session.context.items.length > 0 && <SourcesCard bundle={session.context} />}
        <ol className="learn-timeline" aria-label="Conversation">
          {[...items, ...live].map((item) => <li key={item.key} className="learn-timeline-item"><TimelineEntry item={item} onAnswer={operate === null ? answer : undefined} /></li>)}
        </ol>
        {items.length === 0 && live.length === 0 && turn.pending === null && <p className="learn-muted">Your teacher is getting ready. The conversation shows here.</p>}
        {turn.pending !== null && live.length === 0 && <Working stalled={turn.pending.stalled} onCheck={checkAgain} />}
        {planComplete(session.plan) && <CompletionCard conn={conn} session={session} />}
        {refusal !== null && <RefusalNote refusal={refusal} budget={refusal.kind === "budget" ? budget : null} onRetry={turn.retry ?? (refusal.kind === "budget" ? statusRead.reload : undefined)} />}
        <Composer session={session} busy={operate !== null || turn.pending !== null} paused={pausedBy(budget)} onSend={(text) => turn.act({ cmd: "noesis_send", session: session.id, text })} />
      </div>
      <aside className="learn-aside">
        {session.plan ? <PlanTree plan={session.plan} /> : <p className="learn-muted">The plan appears once Noesis knows where you stand.</p>}
        <UsageNote usage={session.usage} />
        <SessionActions conn={conn} session={session} disabled={operate !== null} />
      </aside>
    </div>
  );
}

function pausedBy(budget: BudgetUse | null): string | null {
  return budget === null ? null : "Teaching is paused: today's budget is spent";
}

function TimelineEntry({ item, onAnswer }: { item: TimelineItem; onAnswer: ((question: string, input: AnswerInput) => Promise<string | null>) | undefined }) {
  if (item.kind === "message") {
    return (
      <div className="learn-bubble" data-role={item.role} data-live={item.live || undefined} aria-busy={item.live || undefined}>
        <span className="ui-visually-hidden">{item.role === "teacher" ? "Teacher:" : "You:"}</span><LessonText text={item.text} />
      </div>
    );
  }
  if (item.kind === "problem") return <ProblemCard problem={item.problem} conclusion={item.conclusion} active={item.active} />;
  const id = item.question.id;
  return <QuestionCard question={item.question} answer={item.answer} result={item.result} open={item.open} onAnswer={onAnswer && ((input) => onAnswer(id, input))} />;
}

function Working({ stalled, onCheck }: { stalled: boolean; onCheck: () => void }) {
  if (stalled) {
    return (
      <div className="learn-callout" role="status">
        <p>No reply yet. The teacher may have stopped, for example at the day's budget.</p>
        <Button size="sm" onClick={onCheck}>Check again</Button>
      </div>
    );
  }
  return <p className="learn-working" role="status"><span className="learn-dots" aria-hidden="true"><span /><span /><span /></span>Your teacher is working…</p>;
}

export default function SessionRoute({ params }: RouteProps) {
  const id = params.id;
  invariant(typeof id === "string" && id.length > 0, "the session route names its session");
  return <LearnFrame view="session" title="Session">{(gate) => <SessionScreen {...gate} id={id} />}</LearnFrame>;
}
