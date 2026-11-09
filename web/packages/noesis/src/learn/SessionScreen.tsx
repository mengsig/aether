// `#/learn/sessions/:id`: the lesson's whole conversation (every agent's, with its questions and
// problems), the plan beside it and the way to answer. Turn activity is shown while replies are sealed.

import type { aether, RouteProps } from "@aether-ui/protocol";
import { useSelector } from "@aether-ui/core";
import { Button, EmptyState, ErrorState, Markdown } from "@aether-ui/ui";
import { useCallback, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useLearn, useNoesisRead, useOperateRefusal } from "./data.ts";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { Composer } from "./Composer.tsx";
import { CompletionCard, SourcesCard } from "./LessonCards.tsx";
import { useLessonTurn } from "./lesson-turn.ts";
import { exhaustedBudget, invariant, PHASE_LABEL, planComplete, type BudgetUse, type Refusal } from "./model.ts";
import { ProblemCard } from "./ProblemCard.tsx";
import { QuestionCard, type AnswerInput } from "./QuestionCard.tsx";
import { RefusalNote } from "./RefusalNote.tsx";
import { SessionAside } from "./SessionAside.tsx";
import { useLessonTranscripts, useLiveText, useTeacherWriting } from "./session-data.ts";
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
  const canRetryTurn = useSelector(useCallback((state) => state.connections[conn]?.features.includes("noesis_retry_turn") === true, [conn]));
  const { transcripts, resumed } = useLessonTranscripts(conn, session);
  const live = useLiveText(conn, session.agent);
  const writing = useTeacherWriting(conn, session.agent) || resumed;
  const items = useMemo(() => buildTimeline(session, transcripts, learn), [session, transcripts, learn]);
  const streamed = live.map((item) => (item.kind === "message" ? item.text : "")).join("");
  const turn = useLessonTurn(conn, session, items, streamed);
  const budget = exhaustedBudget(status);
  const refusal = session.stalled ? stallOrBudgetRefusal(session, budget) : turn.refusal ?? stallOrBudgetRefusal(session, budget);
  const retry = operate !== null || turn.pending !== null ? undefined : session.stalled
    ? (canRetryTurn ? () => void turn.act({ cmd: "noesis_retry_turn", session: session.id }) : undefined)
    : turn.retry ?? undefined;
  const answer = (question: string, input: AnswerInput) => turn.act({ cmd: "noesis_answer", session: session.id, question, answer: input });
  const checkAgain = () => {
    reload();
    turn.unstall();
  };
  return (
    <div className="learn-session">
      <section className="learn-chat" aria-label="Session">
        <header className="learn-chat-head">
          <p className="learn-session-goal">{session.goal}</p>
          <p className="learn-muted">{PHASE_LABEL[session.phase]}</p>
        </header>
        <Transcript items={[...items, ...live]} live={live.length > 0} writing={writing} pending={turn.pending} settingsPending={status.settings_pending === true} onAnswer={operate === null ? answer : undefined} onCheck={checkAgain}>
          {session.context && session.context.items.length > 0 && <SourcesCard bundle={session.context} />}
          {planComplete(session.plan) && <CompletionCard conn={conn} session={session} />}
          {refusal !== null && <RefusalNote refusal={refusal} budget={refusal.kind === "budget" ? budget : null} onRetry={retry} />}
        </Transcript>
        <Composer sessionId={session.id} waiting={session.open_question != null} problem={session.active_problem != null} reason={pausedBy(session.stalled != null || budget !== null)}
          disabled={operate !== null || writing || turn.pending !== null || session.stalled != null || budget !== null} onSend={(text) => turn.act({ cmd: "noesis_send", session: session.id, text })} />
      </section>
      <SessionAside conn={conn} session={session} items={items} disabled={operate !== null} />
    </div>
  );
}

/** A recorded stall speaks first; today's budget is the fallback. */
function stallOrBudgetRefusal(session: aether.NoesisSession, budget: BudgetUse | null): Refusal | null {
  if (session.stalled) return { kind: session.stalled.reason === "engine_stalled" ? "other" : "budget", message: session.stalled.message };
  return budget === null ? null : BUDGET_REFUSAL;
}

function pausedBy(paused: boolean): string | null {
  return paused ? "Teaching is paused: retry the stalled turn" : null;
}

type AnswerFn = (question: string, input: AnswerInput) => Promise<string | null>;

/** Scrolled further than this from the bottom, the chat stops following and offers a jump back. */
const FOLLOW_SLACK_PX = 64;

export function scrolledAway(metrics: { scrollHeight: number; scrollTop: number; clientHeight: number }): boolean {
  invariant(metrics.scrollHeight >= 0 && metrics.clientHeight >= 0, "scroll metrics are nonnegative");
  return metrics.scrollHeight - metrics.scrollTop - metrics.clientHeight > FOLLOW_SLACK_PX;
}

/** Keeps the newest entry in view while the reader is at the bottom; scrolled up, it stays put. */
function useFollowLatest(items: readonly TimelineItem[], pending: object | null) {
  const ref = useRef<HTMLDivElement>(null);
  const [away, setAway] = useState(false);
  const onScroll = () => {
    const node = ref.current;
    if (node !== null) setAway(scrolledAway(node));
  };
  useLayoutEffect(() => {
    const node = ref.current;
    if (node !== null && !away) node.scrollTop = node.scrollHeight;
  }, [items, pending, away]);
  const jump = () => {
    const node = ref.current;
    if (node === null) return;
    node.scrollTop = node.scrollHeight;
    setAway(false);
  };
  return { ref, away, onScroll, jump };
}

interface TranscriptProps {
  readonly items: readonly TimelineItem[];
  readonly live: boolean;
  readonly writing: boolean;
  readonly pending: { readonly stalled: boolean } | null;
  readonly settingsPending: boolean;
  readonly onAnswer: AnswerFn | undefined;
  readonly onCheck: () => void;
  readonly children: ReactNode;
}

function Transcript({ items, live, writing, pending, settingsPending, onAnswer, onCheck, children }: TranscriptProps) {
  const { ref, away, onScroll, jump } = useFollowLatest(items, pending);
  return (
    <div className="learn-transcript">
      <div className="learn-scroll" ref={ref} onScroll={onScroll} data-testid="learn-transcript">
        <ol className="learn-timeline" aria-label="Conversation">
          {items.map((item) => <li key={item.key} className="learn-timeline-item"><TimelineEntry item={item} onAnswer={onAnswer} /></li>)}
        </ol>
        {items.length === 0 && pending === null && !writing && <p className="learn-muted">Your teacher is getting ready. The conversation shows here.</p>}
        {!live && (writing || pending !== null) && (settingsPending && !writing
          ? <p className="learn-working" role="status">Waiting for saved settings to take effect…</p>
          : <Working stalled={!writing && pending?.stalled === true} onCheck={onCheck} />)}
        {children}
      </div>
      {away && <button type="button" className="learn-jump" data-testid="learn-jump-latest" onClick={jump}>Jump to latest ↓</button>}
    </div>
  );
}

function TimelineEntry({ item, onAnswer }: { item: TimelineItem; onAnswer: AnswerFn | undefined }) {
  if (item.kind === "message") {
    return (
      <div className="learn-bubble" data-role={item.role} data-live={item.live || undefined} aria-busy={item.live || undefined}>
        <span className="ui-visually-hidden">{item.role === "teacher" ? "Teacher:" : "You:"}</span><Markdown text={item.text} />
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
  return <p className="learn-working" role="status"><span className="learn-dots" aria-hidden="true"><span /><span /><span /></span>Your teacher is writing…</p>;
}

export default function SessionRoute({ params }: RouteProps) {
  const id = params.id;
  invariant(typeof id === "string" && id.length > 0, "the session route names its session");
  return <LearnFrame view="session" title="Session">{(gate) => <SessionScreen {...gate} id={id} />}</LearnFrame>;
}
