// Beside the chat (below it on narrow screens): the plan, the mastery of what this session asked
// about, what it cost, and the session's own actions.

import type { aether } from "@aether-ui/protocol";
import { useMemo } from "react";
import { useLearn, useNoesisRead } from "./data.ts";
import { UsageNote } from "./LessonCards.tsx";
import { MasteryBadge } from "./Mastery.tsx";
import { invariant, MASTERY_ORDER } from "./model.ts";
import { PlanTree } from "./PlanTree.tsx";
import { SessionActions } from "./SessionActions.tsx";
import type { TimelineItem } from "./timeline.ts";
import { learnerConcepts } from "./UnderstandingScreen.tsx";

/** The learner's concepts this session's questions asked about, what needs work first. */
export function sessionConcepts(items: readonly TimelineItem[], concepts: readonly aether.ConceptState[]): aether.ConceptState[] {
  const asked = new Set(items.flatMap((item) => (item.kind === "question" ? item.question.concepts : [])));
  const found = concepts.filter((concept) => asked.has(concept.concept.id));
  invariant(found.every((concept) => MASTERY_ORDER.includes(concept.state)), "every concept has a known state");
  return found.sort((a, b) => MASTERY_ORDER.indexOf(a.state) - MASTERY_ORDER.indexOf(b.state) || a.concept.title.localeCompare(b.concept.title));
}

export function SessionAside({ conn, session, items, disabled }: { conn: string; session: aether.NoesisSession; items: readonly TimelineItem[]; disabled: boolean }) {
  return (
    <aside className="learn-aside" aria-label="Plan and progress">
      {session.plan ? <PlanTree plan={session.plan} /> : <p className="learn-muted">The plan appears once Noesis knows where you stand.</p>}
      <SessionMastery conn={conn} items={items} />
      <UsageNote usage={session.usage} />
      <SessionActions conn={conn} session={session} disabled={disabled} />
    </aside>
  );
}

function SessionMastery({ conn, items }: { conn: string; items: readonly TimelineItem[] }) {
  useNoesisRead(conn, { kind: "learner" });
  const learner = useLearn(conn).reads.learner;
  const concepts = useMemo(() => sessionConcepts(items, learnerConcepts(learner?.value)), [items, learner]);
  return (
    <section className="learn-session-mastery" aria-label="Mastery">
      <h2 className="learn-subtitle">What you have shown</h2>
      {concepts.length === 0
        ? <p className="learn-muted">Answer a question and what it tested shows here.</p>
        : (
          <ul className="learn-session-concepts">
            {concepts.map((concept) => (
              <li key={concept.concept.id} className="learn-session-concept" data-mastery={concept.state}>
                <span>{concept.concept.title}</span>
                <MasteryBadge state={concept.state} />
              </li>
            ))}
          </ul>
        )}
    </section>
  );
}
