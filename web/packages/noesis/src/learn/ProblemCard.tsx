// A practice problem. The learner works it in the chat; the solution stays sealed until Noesis
// concludes the problem, and the session read or the conclusion event reveals it.

import type { aether } from "@aether-ui/protocol";
import { LessonText } from "./LessonText.tsx";
import { invariant, OUTCOME_LABEL } from "./model.ts";
import type { Conclusion } from "./timeline.ts";

const MODE_LABEL: Readonly<Record<aether.ProblemMode, string>> = {
  recall: "Recall", discrimination: "Tell apart", composition: "Put together",
};

const MISS_LABEL: Readonly<Record<aether.MissClass, string>> = {
  cue: "Missed the cue", discrimination: "Mixed up similar ideas", execution: "Slipped in the working", careless: "A careless slip", structural: "The approach did not fit",
};

export function ProblemCard({ problem, conclusion, active }: { problem: aether.Problem; conclusion: Conclusion | undefined; active: boolean }) {
  invariant(problem.statement.trim() !== "", "a problem states what to solve");
  invariant(conclusion === undefined || conclusion.solution.trim() !== "", "a concluded problem reveals its solution");
  return (
    <article className="learn-q learn-problem" data-state={active && conclusion === undefined ? "open" : "answered"} aria-label="Practice problem">
      <p className="learn-q-kind">Practice · {MODE_LABEL[problem.mode]}</p>
      <p className="learn-q-prompt"><LessonText text={problem.statement} /></p>
      {active && <p className="learn-muted">Work it out below and send your answer. Ask for a hint whenever you need one.</p>}
      {conclusion !== undefined && (
        <div className="learn-reveal" role="status">
          <p className="learn-reveal-verdict" data-correct={conclusion.outcome === "solved" || conclusion.outcome === "solved_with_hints"}>{OUTCOME_LABEL[conclusion.outcome]}</p>
          <p className="learn-muted">
            {conclusion.rung != null ? `Hints used: ${conclusion.rung} of 5` : "No hints"}
            {conclusion.miss_class ? ` · ${MISS_LABEL[conclusion.miss_class]}` : ""}
            {conclusion.retest ? " · Comes back for another try" : ""}
          </p>
          <p><span className="learn-muted">Solution: </span><LessonText text={conclusion.solution} /></p>
        </div>
      )}
      {!active && conclusion === undefined && <p className="learn-muted">Concluded. The solution shows here once Noesis records it.</p>}
    </article>
  );
}
