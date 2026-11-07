// One model call's retrieval: what the model was given, then every candidate with its outcome,
// the policy that decided, the pool size and what the trace's caps left out.

import type { aether } from "@aether-ui/protocol";
import { Badge, type Tone } from "@aether-ui/ui";
import { invariant } from "../fleet/invariant.ts";
import { count, score, share, when } from "./format.ts";

const OUTCOME: Record<aether.TraceOutcome, { label: string; tone: Tone; why: string }> = {
  injected: { label: "Injected", tone: "success", why: "In the model's memory layer" },
  held_out: { label: "Held out", tone: "info", why: "Fit the budget; the holdout drew it" },
  trimmed: { label: "Trimmed", tone: "warning", why: "Within max items; the memory budget had no room" },
  over_limit: { label: "Over limit", tone: "neutral", why: "Passed the cutoff; ranked past max items" },
  cut: { label: "Cut", tone: "neutral", why: "Relevance below min score; never ranked" },
};

type Known = ReadonlyMap<string, string>;

export function ExplainTurn({ turn, known }: { turn: aether.ExplainedTurn; known: Known }) {
  invariant(Number.isInteger(turn.turn) && turn.turn >= 0, "a turn number is a whole number");
  invariant(Number.isInteger(turn.pool) && turn.pool >= 0, "the pool is a count of candidates");
  const injected = turn.injected ?? [];
  return <article className="mem-turn" data-traced={turn.traced ? "true" : "false"} aria-label={`Turn ${turn.turn}`}>
    <header className="mem-turn-head">
      <h4 className="b3-item-title">Turn {turn.turn}</h4>
      <span className="b3-meta">{when(turn.at)} · <code className="b3-code">{turn.snapshot}</code></span>
    </header>
    <div className="mem-turn-block">
      <h5 className="mem-label">Given to the model</h5>
      {injected.length === 0 ? <p className="b3-meta">No memories</p> :
        <ol className="mem-ids">{injected.map(id => <li key={id}><MemoryName id={id} known={known} /></li>)}</ol>}
    </div>
    {turn.traced ? <TracedBody turn={turn} known={known} /> : <p className="mem-untraced" role="note">
      <strong>Untraced.</strong> This snapshot predates retrieval tracing, or the turn did not retrieve.</p>}
  </article>;
}

function TracedBody({ turn, known }: { turn: aether.ExplainedTurn; known: Known }) {
  invariant(turn.traced, "only a traced turn has a policy and candidates to show");
  const candidates = turn.candidates ?? [];
  const omitted = turn.omitted ?? { over_limit: 0, cut: 0 };
  return <>
    <dl className="b3-facts mem-policy">
      <div><dt>Pool</dt><dd>{count(turn.pool, "candidate", "candidates")}</dd></div>
      {turn.policy ? <PolicyFacts policy={turn.policy} /> : null}
      <div><dt>Omitted past the caps</dt><dd>{omitted.over_limit} over limit · {omitted.cut} cut</dd></div>
    </dl>
    <div className="mem-turn-block">
      <h5 className="mem-label">Candidates</h5>
      {candidates.length === 0 ? <p className="b3-meta">No candidates were recorded</p> :
        <ol className="mem-candidates">{candidates.map(c => <CandidateRow key={c.memory} candidate={c} known={known} />)}</ol>}
    </div>
  </>;
}

function PolicyFacts({ policy }: { policy: aether.TracePolicy }) {
  invariant(Number.isInteger(policy.max_items) && policy.max_items > 0, "max items is a positive count");
  return <>
    <div><dt>Max items</dt><dd>{policy.max_items}</dd></div>
    <div><dt>Min score</dt><dd>{policy.min_score == null ? "None (no query)" : score(policy.min_score)}</dd></div>
    <div><dt>Holdout rate</dt><dd>{share(policy.holdout_rate)}</dd></div>
  </>;
}

function CandidateRow({ candidate, known }: { candidate: aether.TraceCandidate; known: Known }) {
  const outcome = OUTCOME[candidate.outcome];
  invariant(outcome !== undefined, `the outcome ${candidate.outcome} is one the contract names`);
  invariant(candidate.outcome !== "cut" || candidate.score == null, "a cut candidate is never scored");
  return <li className="mem-candidate" data-outcome={candidate.outcome}>
    <span className="mem-rank" aria-label={candidate.rank == null ? "Unranked" : `Rank ${candidate.rank}`}>{candidate.rank == null ? "–" : `#${candidate.rank}`}</span>
    <div className="mem-candidate-body">
      <MemoryName id={candidate.memory} known={known} />
      <span className="b3-meta">{outcome.why}</span>
    </div>
    <div className="mem-candidate-facts">
      <Badge tone={outcome.tone}>{outcome.label}</Badge>
      {candidate.pinned ? <Badge tone="accent">Pinned</Badge> : null}
      {candidate.stale ? <Badge tone="warning">Stale · half score</Badge> : null}
      <span className="mem-num">score {candidate.score == null ? "—" : score(candidate.score)}</span>
      <span className="mem-num">relevance {score(candidate.relevance)}</span>
    </div>
  </li>;
}

/** The memory's text when this screen has it loaded, always with its id. */
function MemoryName({ id, known }: { id: string; known: Known }) {
  const text = known.get(id);
  return <span className="mem-name">{text ? <span className="mem-name-text">{text}</span> : null}<code className="b3-code">{id}</code></span>;
}
