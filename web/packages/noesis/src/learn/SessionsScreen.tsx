// `#/learn`: start a session and pick one up again. Sessions come from core's store; the list is
// reread on mount and whenever an event names a session.

import { navigate, routeHref } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Badge, Button, EmptyState, ErrorState } from "@aether-ui/ui";
import { useMemo, useState, type FormEvent } from "react";
import { useLearn, useNoesisChange, useNoesisRead, useOperateRefusal } from "./data.ts";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { classifyRefusal, dateTime, invariant, nextStep, PHASE_LABEL, sortSessions, type Refusal } from "./model.ts";
import { RefusalNote } from "./RefusalNote.tsx";
import { UnderstandFromHash } from "../understand/UnderstandSheet.tsx";

const GOAL_MAX = 2000;

export function SessionsScreen({ conn }: OpenGate) {
  const read = useNoesisRead(conn, { kind: "sessions" });
  const learn = useLearn(conn);
  const listed = learn.reads.sessions !== undefined;
  const sessions = useMemo(() => sortSessions(Object.values(learn.sessions)), [learn.sessions]);
  return (
    <div className="learn-columns">
      <NewSession conn={conn} />
      <section className="learn-list" aria-label="Your sessions">
        <h2 className="learn-subtitle">Your sessions</h2>
        {read.error !== null && sessions.length === 0 && <ErrorState title="Could not load your sessions" message={read.error} onRetry={read.reload} />}
        {read.error === null && !listed && sessions.length === 0 && <p className="learn-muted" aria-busy="true">Loading sessions…</p>}
        {listed && sessions.length === 0 && <EmptyState title="No sessions yet">Name something you want to learn, and Noesis starts by finding out what you already know.</EmptyState>}
        <ul className="learn-cards">
          {sessions.map((session) => <SessionCard key={session.id} conn={conn} session={session} />)}
        </ul>
      </section>
    </div>
  );
}

function SessionCard({ conn, session }: { conn: string; session: aether.NoesisSession }) {
  const href = routeHref("#/learn/sessions/:id", { id: session.id });
  const waiting = session.open_question != null || session.active_problem != null;
  invariant(conn.length > 0, "a session card belongs to a connection");
  return (
    <li className="learn-card">
      <a className="learn-card-link" href={href}>
        <span className="learn-card-goal">{session.goal}</span>
        <span className="learn-card-meta">
          <Badge tone={session.mode === "practice" ? "info" : "accent"}>{session.mode === "practice" ? "Practice" : "Learn"}</Badge>
          <span>{PHASE_LABEL[session.phase]}</span>
          <span aria-hidden="true">·</span>
          <span>{dateTime(session.updated_at)}</span>
        </span>
        <span className="learn-card-next" data-waiting={waiting || undefined}>{nextStep(session)}</span>
      </a>
    </li>
  );
}

function NewSession({ conn }: { conn: string }) {
  const change = useNoesisChange(conn);
  const refusal = useOperateRefusal(conn);
  const [goal, setGoal] = useState("");
  const [mode, setMode] = useState<aether.SessionMode>("teach");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Refusal | null>(null);
  const trimmed = goal.trim();
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (trimmed === "" || trimmed.length > GOAL_MAX) return;
    setBusy(true);
    const result = await change({ cmd: "noesis_create_session", goal: trimmed, mode });
    setBusy(false);
    if (!result.ok) return setFailure(classifyRefusal(result.error));
    const data = result.reply.data as { id?: unknown } | undefined;
    invariant(typeof data?.id === "string", "noesis_create_session answers with the new session");
    navigate(routeHref("#/learn/sessions/:id", { id: data.id }));
  };
  return (
    <form className="learn-new" onSubmit={(event) => void submit(event)} aria-label="Start a session">
      <h2 className="learn-subtitle">What do you want to learn?</h2>
      <label className="learn-visually-hidden" htmlFor="learn-goal">Your goal</label>
      <textarea id="learn-goal" className="learn-textarea" rows={3} maxLength={GOAL_MAX} placeholder="For example: how TCP congestion control reacts to loss"
        value={goal} onChange={(e) => setGoal(e.target.value)} />
      <div className="learn-segmented" role="radiogroup" aria-label="Kind of session">
        {(["teach", "practice"] as const).map((value) => (
          <label key={value} className="learn-segment">
            <input type="radio" name="learn-mode" value={value} checked={mode === value} onChange={() => setMode(value)} />
            <span>{value === "teach" ? "Learn something new" : "Practise what I know"}</span>
          </label>
        ))}
      </div>
      {failure !== null && <RefusalNote refusal={failure} />}
      <div className="learn-actions">
        <Button variant="primary" type="submit" disabled={busy || trimmed === "" || refusal !== null} title={refusal ?? undefined}>{busy ? "Starting…" : "Start session"}</Button>
      </div>
    </form>
  );
}

export default function SessionsRoute() {
  return <><LearnFrame view="sessions">{(gate) => <SessionsScreen {...gate} />}</LearnFrame><UnderstandFromHash /></>;
}
