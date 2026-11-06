// Beside the chat (below it on narrow screens): the plan, the mastery of what this session asked
// about, and the session's own actions.

import { navigate } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, Dialog } from "@aether-ui/ui";
import { useMemo, useState } from "react";
import { useLearn, useNoesisChange, useNoesisRead } from "./data.ts";
import { MasteryBadge } from "./Mastery.tsx";
import { invariant, MASTERY_ORDER, type TimelineItem } from "./model.ts";
import { PlanTree } from "./PlanTree.tsx";
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
