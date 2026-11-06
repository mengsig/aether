// One turn of a lesson: the learner speaks or answers, and the chat waits for the teacher. A refused
// turn never waits; it keeps its command so the learner can try it again once the cause is fixed.

import type { aether } from "@aether-ui/protocol";
import { useEffect, useState } from "react";
import { useNoesisChange } from "./data.ts";
import { classifyRefusal, invariant, type Refusal } from "./model.ts";
import type { TimelineItem } from "./timeline.ts";

/** After this long without a reply the chat says so and offers to check again. */
export const STALL_MS = 90_000;

interface Progress {
  readonly teacher: number;
  readonly question: string | null;
  readonly problem: string | null;
}

interface Pending extends Progress {
  readonly stalled: boolean;
}

/** Where the session stood when the learner spoke: a newer teacher message, question or problem ends the wait. */
function progressOf(session: aether.NoesisSession, items: readonly TimelineItem[]): Progress {
  return {
    teacher: items.filter((item) => item.kind === "message" && item.role === "teacher" && !item.live).length,
    question: session.open_question?.id ?? null,
    problem: session.active_problem?.id ?? null,
  };
}

function answered(pending: Pending, now: Progress): boolean {
  return now.teacher > pending.teacher || (now.question !== null && now.question !== pending.question) || (now.problem !== null && now.problem !== pending.problem);
}

export interface LessonTurn {
  readonly pending: { readonly stalled: boolean } | null;
  readonly refusal: Refusal | null;
  readonly act: (command: Record<string, unknown>) => Promise<string | null>;
  /** Sends the refused command again; null when nothing was refused. */
  readonly retry: (() => void) | null;
  readonly unstall: () => void;
}

/** `streamed` is the teacher's live text so far: while it grows, the wait is not stalled. */
export function useLessonTurn(conn: string, session: aether.NoesisSession, items: readonly TimelineItem[], streamed: string): LessonTurn {
  const change = useNoesisChange(conn);
  const progress = progressOf(session, items);
  const [pending, setPending] = useState<Pending | null>(null);
  const [refused, setRefused] = useState<{ refusal: Refusal; command: Record<string, unknown> } | null>(null);

  if (pending !== null && answered(pending, progress)) setPending(null);
  useEffect(() => {
    if (pending === null || pending.stalled) return;
    const timer = setTimeout(() => setPending((old) => (old === null ? null : { ...old, stalled: true })), STALL_MS);
    return () => clearTimeout(timer);
  }, [pending, streamed]);

  const act = async (command: Record<string, unknown>): Promise<string | null> => {
    invariant(command.session === session.id, "a lesson's turn speaks to its own session");
    const before = progress;
    const result = await change(command);
    if (!result.ok) {
      setRefused({ refusal: classifyRefusal(result.error), command });
      setPending(null);
      return result.error;
    }
    setRefused(null);
    setPending({ ...before, stalled: false });
    return null;
  };
  const retry = refused === null ? null : () => void act(refused.command);
  const unstall = () => setPending((old) => (old === null ? null : { ...old, stalled: false }));
  return { pending, refusal: refused?.refusal ?? null, act, retry, unstall };
}
