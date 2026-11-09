// `#/learn`: start a session and pick one up again. Sessions come from core's store; the list is
// reread on mount and whenever an event names a session.

import { navigate, routeHref, useDraft } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Badge, Button, EmptyState, ErrorState, Segmented, TextArea } from "@aether-ui/ui";
import { useMemo, useState, type FormEvent } from "react";
import { useLearn, useNoesisChange, useNoesisRead, useOperateRefusal } from "./data.ts";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { classifyRefusal, dateTime, draftKey, invariant, nextStep, PHASE_LABEL, planProgress, recentGoals, sortSessions, type Refusal } from "./model.ts";
import { RefusalNote } from "./RefusalNote.tsx";
import { CatchUpEntry } from "../understand/CatchUpSheet.tsx";
import { UnderstandFromHash } from "../understand/UnderstandSheet.tsx";

const GOAL_MAX = 2000;

export const MODES: readonly { readonly id: aether.SessionMode; readonly label: string; readonly hint: string }[] = [
  { id: "teach", label: "Learn something new", hint: "Finds what you know, then teaches step by step" },
  { id: "practice", label: "Practise what I know", hint: "Problems at the edge of what you can do" },
];

const EMPTY_GOAL: { readonly goal: string; readonly mode: aether.SessionMode } = { goal: "", mode: "teach" };

export function SessionsScreen({ conn }: OpenGate) {
  const read = useNoesisRead(conn, { kind: "all-sessions" });
  const learn = useLearn(conn);
  const listed = learn.reads["all-sessions"] !== undefined;
  const sessions = useMemo(() => sortSessions(((learn.reads["all-sessions"]?.value.ids as string[] | undefined) ?? []).flatMap((id) => learn.sessions[id] ? [learn.sessions[id]] : [])), [learn.sessions, learn.reads]);
  return (
    <div className="learn-columns">
      <div className="learn-start">
        <NewSession conn={conn} suggestions={recentGoals(sessions)} />
        <CatchUpEntry />
      </div>
      <section className="learn-list" aria-label="Your sessions">
        <h2 className="learn-subtitle">Your sessions</h2>
        {read.error !== null && <ErrorState title="Could not load your sessions" message={read.error} onRetry={read.reload} />}
        {read.error === null && (read.loading || !listed) && <p className="learn-muted" aria-busy="true">Loading sessions…</p>}
        {listed && !read.loading && read.error === null && sessions.length === 0 && (
          <EmptyState title="No sessions yet">Name something you want to learn on the left, and Noesis starts by finding out what you already know.</EmptyState>
        )}
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
  const progress = planProgress(session);
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
        {progress !== null && (
          <span className="learn-card-progress">
            <meter className="learn-meter" min={0} max={progress.total} value={progress.done} aria-label={`Progress on ${session.goal}`} />
            <span className="learn-num">{progress.done} of {progress.total} steps</span>
          </span>
        )}
        <span className="learn-card-next" data-waiting={waiting || undefined}>{nextStep(session)}</span>
      </a>
    </li>
  );
}

/** Why Start is off, said beside it; null when it may be pressed. */
function startBlocker(goal: string, refusal: string | null): string | null {
  if (refusal !== null) return refusal;
  if (goal === "") return "Name a goal to start";
  if (goal.length > GOAL_MAX) return `Keep the goal under ${GOAL_MAX} characters`;
  return null;
}

function NewSession({ conn, suggestions }: { conn: string; suggestions: readonly string[] }) {
  const change = useNoesisChange(conn);
  const refusal = useOperateRefusal(conn);
  const draft = useDraft(draftKey("new-session", conn), EMPTY_GOAL);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Refusal | null>(null);
  const goal = draft.value.goal;
  const mode = draft.value.mode;
  invariant(MODES.some((option) => option.id === mode), "the draft holds a known mode");
  const trimmed = goal.trim();
  const blocker = startBlocker(trimmed, refusal);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (blocker !== null) return;
    setBusy(true);
    const result = await change({ cmd: "noesis_create_session", goal: trimmed, mode });
    setBusy(false);
    if (!result.ok) return setFailure(classifyRefusal(result.error));
    const data = result.reply.data as { id?: unknown } | undefined;
    invariant(typeof data?.id === "string", "noesis_create_session answers with the new session");
    draft.clearIfUnchanged();
    navigate(routeHref("#/learn/sessions/:id", { id: data.id }));
  };
  return (
    <form className="learn-new" onSubmit={(event) => void submit(event)} aria-label="Start a session">
      <h2 className="learn-subtitle">What do you want to learn?</h2>
      <TextArea label="Your goal" hideLabel data-testid="learn-goal" rows={3} maxLength={GOAL_MAX} autoFocus placeholder="For example: how TCP congestion control reacts to loss"
        value={goal} onChange={(event) => draft.update({ goal: event.target.value })} />
      {suggestions.length > 0 && trimmed === "" && <GoalSuggestions goals={suggestions} onPick={(picked) => draft.update({ goal: picked })} />}
      <div data-testid="learn-mode">
        <Segmented label="Kind of session" layout="cards" options={MODES} value={mode} onChange={(id) => draft.update({ mode: id as aether.SessionMode })} />
      </div>
      {failure !== null && <RefusalNote refusal={failure} />}
      <div className="learn-actions">
        <Button variant="primary" type="submit" data-testid="learn-start" disabled={busy || blocker !== null} aria-describedby={blocker === null ? undefined : "learn-start-why"}>
          {busy ? "Starting…" : "Start session"}
        </Button>
        {blocker !== null && <span className="learn-muted" id="learn-start-why">{blocker}</span>}
      </div>
    </form>
  );
}

function GoalSuggestions({ goals, onPick }: { goals: readonly string[]; onPick: (goal: string) => void }) {
  invariant(goals.length > 0, "suggestions are offered only when there are some");
  return (
    <div className="learn-suggestions" data-testid="learn-goal-suggestions">
      <span className="learn-muted">Pick up a recent goal:</span>
      {goals.map((goal) => <button key={goal} type="button" className="learn-chip" onClick={() => onPick(goal)}>{goal}</button>)}
    </div>
  );
}

export default function SessionsRoute() {
  return <><LearnFrame view="sessions">{(gate) => <SessionsScreen {...gate} />}</LearnFrame><UnderstandFromHash /></>;
}
