// A practice problem. The learner works it in the chat; the solution stays sealed until Noesis
// concludes the problem, and only that event reveals it.

import type { aether } from "@aether-ui/protocol";
import { invariant, OUTCOME_LABEL } from "./model.ts";

const MODE_LABEL: Readonly<Record<aether.ProblemMode, string>> = {
  recall: "Recall", discrimination: "Tell apart", composition: "Put together",
};

const MISS_LABEL: Readonly<Record<aether.MissClass, string>> = {
  cue: "Missed the cue", discrimination: "Mixed up similar ideas", execution: "Slipped in the working", careless: "A careless slip", structural: "The approach did not fit",
};

export function ProblemCard({ problem, conclusion, active }: { problem: aether.Problem; conclusion: aether.EventKindNoesisPracticeConcluded | undefined; active: boolean }) {
  invariant(conclusion === undefined || conclusion.problem === problem.id, "a conclusion belongs to its problem");
  return (
    <article className="learn-q learn-problem" data-state={active ? "open" : "answered"} aria-label="Practice problem">
      <p className="learn-q-kind">Practice · {MODE_LABEL[problem.mode]}</p>
      <p className="learn-q-prompt">{problem.statement}</p>
      {active && <p className="learn-muted">Work it out below and send your answer. Ask for a hint whenever you need one.</p>}
      {conclusion !== undefined && (
        <div className="learn-reveal" role="status">
          <p className="learn-reveal-verdict" data-correct={conclusion.outcome === "solved" || conclusion.outcome === "solved_with_hints"}>{OUTCOME_LABEL[conclusion.outcome]}</p>
          <p className="learn-muted">
            {conclusion.rung != null ? `Hints used: ${conclusion.rung} of 5` : "No hints"}
            {conclusion.miss_class ? ` · ${MISS_LABEL[conclusion.miss_class]}` : ""}
            {conclusion.retest ? " · Comes back for another try" : ""}
          </p>
          <p><span className="learn-muted">Solution: </span>{conclusion.solution}</p>
        </div>
      )}
      {!active && conclusion === undefined && <p className="learn-muted">Concluded. The solution shows here once Noesis records it.</p>}
    </article>
  );
}
