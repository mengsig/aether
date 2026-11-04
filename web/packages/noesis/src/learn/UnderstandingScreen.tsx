// `#/learn/me`: what Noesis has seen you show, from `noesis_learner`. Each concept carries its mastery
// state as a token colour, a mark and a text label; what needs work comes first.

import type { aether } from "@aether-ui/protocol";
import { EmptyState, ErrorState } from "@aether-ui/ui";
import { useMemo } from "react";
import { useLearn, useNoesisRead } from "./data.ts";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { MasteryBadge } from "./Mastery.tsx";
import { dateTime, invariant, isMastery, MASTERY, MASTERY_ORDER, type Mastery } from "./model.ts";

/** The learner read, checked at the boundary: every concept has a known state and a strength in 0..1. */
export function learnerConcepts(value: Record<string, unknown> | undefined): aether.ConceptState[] {
  if (value === undefined) return [];
  invariant(Array.isArray(value.concepts), "noesis_learner carries concepts");
  const concepts = value.concepts as aether.ConceptState[];
  for (const concept of concepts) {
    invariant(isMastery(concept.state), `concept ${concept.concept.id} has a known state`);
    invariant(concept.strength >= 0 && concept.strength <= 1, `concept ${concept.concept.id} has a strength in 0..1`);
  }
  return concepts;
}

export function groupByMastery(concepts: readonly aether.ConceptState[]): Map<Mastery, aether.ConceptState[]> {
  const groups = new Map<Mastery, aether.ConceptState[]>(MASTERY_ORDER.map((state) => [state, []]));
  for (const concept of concepts) groups.get(concept.state)?.push(concept);
  for (const list of groups.values()) list.sort((a, b) => a.concept.title.localeCompare(b.concept.title));
  return groups;
}

export function UnderstandingScreen({ conn }: OpenGate) {
  const read = useNoesisRead(conn, { kind: "learner" });
  const learn = useLearn(conn);
  const held = learn.reads.learner;
  const concepts = useMemo(() => learnerConcepts(held?.value), [held]);
  const groups = useMemo(() => groupByMastery(concepts), [concepts]);
  if (held === undefined) {
    if (read.error !== null) return <ErrorState title="Could not read your understanding" message={read.error} onRetry={read.reload} />;
    return <p className="learn-muted" aria-busy="true">Reading what you know…</p>;
  }
  if (concepts.length === 0) {
    return <EmptyState title="Nothing recorded yet">Answer questions in a session, and Noesis keeps track of what you show here. Seeing something is never counted as knowing it.</EmptyState>;
  }
  return (
    <div className="learn-me">
      <ul className="learn-me-summary" aria-label="Concepts by state">
        {MASTERY_ORDER.map((state) => (
          <li key={state} className="learn-me-count"><span className="learn-num">{groups.get(state)?.length ?? 0}</span><MasteryBadge state={state} /></li>
        ))}
      </ul>
      {read.error !== null && <p className="learn-callout" role="alert">Showing the last read: {read.error}</p>}
      {MASTERY_ORDER.filter((state) => (groups.get(state)?.length ?? 0) > 0).map((state) => (
        <section key={state} className="learn-me-group" aria-label={MASTERY[state].label}>
          <h2 className="learn-subtitle">{MASTERY[state].label} <span className="learn-muted">{MASTERY[state].hint}</span></h2>
          <ul className="learn-me-list">
            {(groups.get(state) ?? []).map((concept) => <ConceptRow key={concept.concept.id} concept={concept} />)}
          </ul>
        </section>
      ))}
    </div>
  );
}

function ConceptRow({ concept }: { concept: aether.ConceptState }) {
  const strength = Math.round(concept.strength * 100);
  return (
    <li className="learn-concept" data-mastery={concept.state}>
      <div className="learn-concept-head">
        <span className="learn-concept-title">{concept.concept.title}</span>
        <MasteryBadge state={concept.state} />
      </div>
      <div className="learn-concept-facts">
        <meter className="learn-meter" data-mastery={concept.state} min={0} max={100} value={strength} aria-label={`Strength of ${concept.concept.title}`} />
        <span className="learn-num">{strength}% strength</span>
        {concept.review_due_at && <span>Review {dateTime(concept.review_due_at)}</span>}
        {concept.calibration != null && <span title="Brier score of your stated confidence; lower is better. Shown, never graded.">Calibration {concept.calibration.toFixed(2)}</span>}
      </div>
      {concept.misconceptions.length > 0 && (
        <ul className="learn-misconceptions" aria-label="Misconceptions to clear">
          {concept.misconceptions.map((text) => <li key={text}>{text}</li>)}
        </ul>
      )}
    </li>
  );
}

export default function UnderstandingRoute() {
  return <LearnFrame view="me" title="My understanding">{(gate) => <UnderstandingScreen {...gate} />}</LearnFrame>;
}
