// `#/learn/sessions/:id`: the conversation with its questions and problems, the plan beside it, and the
// way to answer. Teaching has no streaming yet: after the learner speaks the chat shows the teacher is
// working until the committed reply arrives, which the session's reread and the transcript bring.

import { selectTranscript, useApp, useSelector } from "@aether-ui/core";
import type { aether, RouteProps } from "@aether-ui/protocol";
import { Button, EmptyState, ErrorState } from "@aether-ui/ui";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Composer } from "./Composer.tsx";
import { errorText, useLearn, useNoesisChange, useNoesisRead, useOperateRefusal } from "./data.ts";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { buildTimeline, classifyRefusal, invariant, PHASE_LABEL, type Refusal, type TimelineItem } from "./model.ts";
import { ProblemCard } from "./ProblemCard.tsx";
import { QuestionCard, type AnswerInput } from "./QuestionCard.tsx";
import { RefusalNote } from "./RefusalNote.tsx";
import { SessionAside } from "./SessionAside.tsx";

/** After this long without a reply the chat says so and offers to check again. */
export const STALL_MS = 90_000;

interface Pending {
  readonly teacher: number;
  readonly question: string | null;
  readonly problem: string | null;
  readonly stalled: boolean;
}

/** Where the session stood when the learner spoke: a newer teacher message, question or problem ends the wait. */
function progressOf(session: aether.NoesisSession, items: readonly TimelineItem[]): Omit<Pending, "stalled"> {
  return {
    teacher: items.filter((item) => item.kind === "message" && item.role === "teacher").length,
    question: session.open_question?.id ?? null,
    problem: session.active_problem?.id ?? null,
  };
}

function answered(pending: Pending, now: Omit<Pending, "stalled">): boolean {
  return now.teacher > pending.teacher || (now.question !== null && now.question !== pending.question) || (now.problem !== null && now.problem !== pending.problem);
}

export function SessionScreen({ conn, id }: OpenGate & { id: string }) {
  const read = useNoesisRead(conn, { kind: "session", id });
  const learn = useLearn(conn);
  const session = learn.sessions[id];
  if (session === undefined) {
    if (learn.deleted.includes(id)) return <EmptyState title="Session deleted" action={<a className="ui-link" href="#/learn">All sessions</a>}>Its conversation stays in Aether history.</EmptyState>;
    if (read.error !== null) return <ErrorState title="Could not open this session" message={read.error} onRetry={read.reload} />;
    return <p className="learn-muted" aria-busy="true">Opening the session…</p>;
  }
  return <SessionView conn={conn} session={session} reload={read.reload} />;
}

/** Deltas and transcript rereads for the session's agent: core keeps the conversation current. */
function useWatchAgent(conn: string, agent: string) {
  invariant(conn.length > 0 && agent.length > 0, "a session's conversation is followed on a connection, by its agent");
  const app = useApp();
  useEffect(() => {
    const watch = (agents: readonly string[]) => app.manager.watchAgents(conn, agents).catch((error: unknown) => {
      app.store.report(`Could not follow the session's conversation: ${errorText(error)}`, conn);
    });
    void watch([agent]);
    return () => void watch([]);
  }, [app, conn, agent]);
}

function SessionView({ conn, session, reload }: { conn: string; session: aether.NoesisSession; reload: () => void }) {
  const learn = useLearn(conn);
  const change = useNoesisChange(conn);
  const refusal = useOperateRefusal(conn);
  const transcript = useSelector(useMemo(() => selectTranscript(conn, session.agent), [conn, session.agent]));
  const items = useMemo(() => buildTimeline(session, transcript?.entries ?? [], learn), [session, transcript, learn]);
  const progress = progressOf(session, items);
  const [pending, setPending] = useState<Pending | null>(null);
  const [failure, setFailure] = useState<Refusal | null>(null);

  useWatchAgent(conn, session.agent);

  if (pending !== null && answered(pending, progress)) setPending(null);
  useEffect(() => {
    if (pending === null || pending.stalled) return;
    const timer = setTimeout(() => setPending((old) => (old === null ? null : { ...old, stalled: true })), STALL_MS);
    return () => clearTimeout(timer);
  }, [pending]);

  const act = async (command: Record<string, unknown>): Promise<string | null> => {
    const before = progress;
    const result = await change(command);
    if (!result.ok) {
      setFailure(classifyRefusal(result.error));
      return result.error;
    }
    setFailure(null);
    setPending({ ...before, stalled: false });
    return null;
  };
  const answer = (question: string, input: AnswerInput) => act({ cmd: "noesis_answer", session: session.id, question, answer: input });
  const checkAgain = () => {
    reload();
    setPending((old) => (old === null ? null : { ...old, stalled: false }));
  };

  return (
    <div className="learn-session">
      <section className="learn-chat" aria-label="Session">
        <header className="learn-chat-head">
          <p className="learn-session-goal">{session.goal}</p>
          <p className="learn-muted">{PHASE_LABEL[session.phase]}</p>
        </header>
        <Transcript items={items} pending={pending} failure={failure} onAnswer={refusal === null ? answer : undefined} onCheck={checkAgain} />
        <Composer sessionId={session.id} waiting={session.open_question != null} problem={session.active_problem != null}
          disabled={refusal !== null || pending !== null} onSend={(text) => act({ cmd: "noesis_send", session: session.id, text })} />
      </section>
      <SessionAside conn={conn} session={session} items={items} disabled={refusal !== null} />
    </div>
  );
}

type AnswerFn = (question: string, input: AnswerInput) => Promise<string | null>;

/** Scrolled further than this from the bottom, the chat stops following and offers a jump back. */
const FOLLOW_SLACK_PX = 64;

export function scrolledAway(metrics: { scrollHeight: number; scrollTop: number; clientHeight: number }): boolean {
  invariant(metrics.scrollHeight >= 0 && metrics.clientHeight >= 0, "scroll metrics are nonnegative");
  return metrics.scrollHeight - metrics.scrollTop - metrics.clientHeight > FOLLOW_SLACK_PX;
}

/** Keeps the newest entry in view while the reader is at the bottom; scrolled up, it stays put. */
function useFollowLatest(items: readonly TimelineItem[], pending: Pending | null, failure: Refusal | null) {
  const ref = useRef<HTMLDivElement>(null);
  const [away, setAway] = useState(false);
  const onScroll = () => {
    const node = ref.current;
    if (node !== null) setAway(scrolledAway(node));
  };
  useLayoutEffect(() => {
    const node = ref.current;
    if (node !== null && !away) node.scrollTop = node.scrollHeight;
  }, [items, pending, failure, away]);
  const jump = () => {
    const node = ref.current;
    if (node === null) return;
    node.scrollTop = node.scrollHeight;
    setAway(false);
  };
  return { ref, away, onScroll, jump };
}

function Transcript({ items, pending, failure, onAnswer, onCheck }: { items: readonly TimelineItem[]; pending: Pending | null; failure: Refusal | null; onAnswer: AnswerFn | undefined; onCheck: () => void }) {
  const { ref, away, onScroll, jump } = useFollowLatest(items, pending, failure);
  return (
    <div className="learn-transcript">
      <div className="learn-scroll" ref={ref} onScroll={onScroll} data-testid="learn-transcript">
        <ol className="learn-timeline" aria-label="Conversation">
          {items.map((item) => <li key={item.key} className="learn-timeline-item"><TimelineEntry item={item} onAnswer={onAnswer} /></li>)}
        </ol>
        {items.length === 0 && pending === null && <p className="learn-muted">Your teacher is getting ready. The conversation shows here.</p>}
        {pending !== null && <Working stalled={pending.stalled} onCheck={onCheck} />}
        {failure !== null && <RefusalNote refusal={failure} />}
      </div>
      {away && <button type="button" className="learn-jump" data-testid="learn-jump-latest" onClick={jump}>Jump to latest ↓</button>}
    </div>
  );
}

function TimelineEntry({ item, onAnswer }: { item: TimelineItem; onAnswer: AnswerFn | undefined }) {
  if (item.kind === "message") {
    return <div className="learn-bubble" data-role={item.role}><span className="ui-visually-hidden">{item.role === "teacher" ? "Teacher:" : "You:"}</span>{item.text}</div>;
  }
  if (item.kind === "problem") return <ProblemCard problem={item.problem} conclusion={item.conclusion} active={item.active} />;
  const id = item.question.id;
  return <QuestionCard question={item.question} result={item.result} open={item.open} onAnswer={onAnswer && ((input) => onAnswer(id, input))} />;
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
