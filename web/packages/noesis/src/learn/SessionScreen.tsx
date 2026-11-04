// `#/learn/sessions/:id`: the conversation with its questions and problems, the plan beside it, and the
// way to answer. Teaching has no streaming yet: after the learner speaks the chat shows the teacher is
// working until the committed reply arrives, which the session's reread and the transcript bring.

import { navigate, selectTranscript, useApp, useSelector } from "@aether-ui/core";
import type { aether, RouteProps } from "@aether-ui/protocol";
import { Button, Dialog, EmptyState, ErrorState } from "@aether-ui/ui";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import { errorText, useLearn, useNoesisChange, useNoesisRead, useOperateRefusal } from "./data.ts";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { buildTimeline, classifyRefusal, invariant, PHASE_LABEL, type Refusal, type TimelineItem } from "./model.ts";
import { PlanTree } from "./PlanTree.tsx";
import { ProblemCard } from "./ProblemCard.tsx";
import { QuestionCard, type AnswerInput } from "./QuestionCard.tsx";
import { RefusalNote } from "./RefusalNote.tsx";

/** After this long without a reply the chat says so and offers to check again. */
export const STALL_MS = 90_000;
const TEXT_MAX = 20_000;

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

function SessionView({ conn, session, reload }: { conn: string; session: aether.NoesisSession; reload: () => void }) {
  const app = useApp();
  const learn = useLearn(conn);
  const change = useNoesisChange(conn);
  const refusal = useOperateRefusal(conn);
  const transcript = useSelector(useMemo(() => selectTranscript(conn, session.agent), [conn, session.agent]));
  const items = useMemo(() => buildTimeline(session, transcript?.entries ?? [], learn), [session, transcript, learn]);
  const progress = progressOf(session, items);
  const [pending, setPending] = useState<Pending | null>(null);
  const [failure, setFailure] = useState<Refusal | null>(null);

  useEffect(() => {
    // Deltas and transcript rereads for the session's agent: core keeps the conversation current.
    const watch = (agents: readonly string[]) => app.manager.watchAgents(conn, agents).catch((error: unknown) => {
      app.store.report(`Could not follow the session's conversation: ${errorText(error)}`, conn);
    });
    void watch([session.agent]);
    return () => void watch([]);
  }, [app, conn, session.agent]);

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
      <div className="learn-chat">
        <p className="learn-session-goal">{session.goal}</p>
        <p className="learn-muted">{PHASE_LABEL[session.phase]}</p>
        <ol className="learn-timeline" aria-label="Conversation">
          {items.map((item) => <li key={item.key} className="learn-timeline-item"><TimelineEntry item={item} onAnswer={refusal === null ? answer : undefined} /></li>)}
        </ol>
        {items.length === 0 && pending === null && <p className="learn-muted">Your teacher is getting ready. The conversation shows here.</p>}
        {pending !== null && <Working stalled={pending.stalled} onCheck={checkAgain} />}
        {failure !== null && <RefusalNote refusal={failure} />}
        <Composer session={session} disabled={refusal !== null || pending !== null} onSend={(text) => act({ cmd: "noesis_send", session: session.id, text })} />
      </div>
      <aside className="learn-aside">
        {session.plan ? <PlanTree plan={session.plan} /> : <p className="learn-muted">The plan appears once Noesis knows where you stand.</p>}
        <SessionActions conn={conn} session={session} disabled={refusal !== null} />
      </aside>
    </div>
  );
}

function TimelineEntry({ item, onAnswer }: { item: TimelineItem; onAnswer: ((question: string, input: AnswerInput) => Promise<string | null>) | undefined }) {
  if (item.kind === "message") {
    return <div className="learn-bubble" data-role={item.role}><span className="learn-visually-hidden">{item.role === "teacher" ? "Teacher:" : "You:"}</span>{item.text}</div>;
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

function Composer({ session, disabled, onSend }: { session: aether.NoesisSession; disabled: boolean; onSend: (text: string) => Promise<string | null> }) {
  const [text, setText] = useState("");
  const waiting = session.open_question != null;
  const trimmed = text.trim();
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (trimmed === "" || waiting) return;
    if ((await onSend(trimmed)) === null) setText("");
  };
  return (
    <form className="learn-composer" onSubmit={(event) => void submit(event)}>
      <label className="learn-visually-hidden" htmlFor="learn-reply">Message your teacher</label>
      <textarea id="learn-reply" className="learn-textarea" rows={2} maxLength={TEXT_MAX} disabled={disabled || waiting}
        placeholder={waiting ? "Answer the question above to go on" : session.active_problem ? "Your answer, or ask for a hint" : "Ask, answer or say what is unclear"}
        value={text} onChange={(e) => setText(e.target.value)} />
      <Button variant="primary" type="submit" disabled={disabled || waiting || trimmed === ""}>Send</Button>
    </form>
  );
}

function SessionActions({ conn, session, disabled }: { conn: string; session: aether.NoesisSession; disabled: boolean }) {
  const change = useNoesisChange(conn);
  const [confirming, setConfirming] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  invariant(session.id.length > 0, "actions name their session");
  const extract = async () => {
    const result = await change({ cmd: "noesis_extract", session: session.id });
    setNote(result.ok ? "Queued. The knowledge graph updates when the job finishes." : result.error);
  };
  const remove = async () => {
    const result = await change({ cmd: "noesis_delete_session", session: session.id, graph: "preserve" });
    if (!result.ok) return setNote(result.error);
    setConfirming(false);
    navigate("#/learn");
  };
  return (
    <section className="learn-session-actions" aria-label="Session actions">
      <Button disabled={disabled} onClick={() => void extract()}>Add to knowledge graph</Button>
      <Button variant="danger" disabled={disabled} onClick={() => setConfirming(true)}>Delete session…</Button>
      {note !== null && <p className="learn-muted" role="status">{note}</p>}
      <Dialog open={confirming} onClose={() => setConfirming(false)} title="Delete this session?"
        description="Noesis forgets this session: its plan, questions, problems and the evidence attached to it."
        footer={<><Button onClick={() => setConfirming(false)} data-autofocus>Keep it</Button><Button variant="danger" onClick={() => void remove()}>Delete session</Button></>}>
        <p>The conversation itself stays in Aether history, where retention removes it in time. What the knowledge graph learned from it stays too.</p>
      </Dialog>
    </section>
  );
}

export default function SessionRoute({ params }: RouteProps) {
  const id = params.id;
  invariant(typeof id === "string" && id.length > 0, "the session route names its session");
  return <LearnFrame view="session" title="Session">{(gate) => <SessionScreen {...gate} id={id} />}</LearnFrame>;
}
