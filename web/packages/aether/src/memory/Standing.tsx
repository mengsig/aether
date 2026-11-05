// What admission and retrieval know about a memory (MEMORY-V2.md §1): tier, state, support,
// verification, refs and a stale note. An inferred memory is an agent's claim, never a fact.

import type { aether } from "@aether-ui/protocol";
import { Badge, type Tone } from "@aether-ui/ui";
import { invariant } from "../fleet/invariant.ts";
import { count, when } from "./format.ts";

const STATE: Record<aether.MemoryState, { label: string; tone: Tone }> = {
  active: { label: "Active", tone: "success" },
  candidate: { label: "Candidate", tone: "info" },
  superseded: { label: "Superseded", tone: "neutral" },
  retracted: { label: "Retracted", tone: "danger" },
};

const VERIFICATION: Record<aether.Verification, { label: string; tone: Tone }> = {
  inferred: { label: "Inferred · unverified", tone: "warning" },
  observed: { label: "Observed", tone: "neutral" },
  verified: { label: "Verified", tone: "success" },
};

export function StandingLine({ standing }: { standing: aether.MemoryStanding }) {
  const state = STATE[standing.state];
  const verification = VERIFICATION[standing.verification];
  invariant(state !== undefined && verification !== undefined, "the standing's state and verification are ones the contract names");
  const refs = standing.refs ?? [];
  return <div className="mem-standing" data-state={standing.state} data-verification={standing.verification}>
    <div className="b3-row mem-tight">
      <Badge tone={state.tone}>{state.label}</Badge>
      <Badge tone={verification.tone}>{verification.label}</Badge>
      <span className="b3-meta">Tier {standing.tier} · {supportText(standing.support)}</span>
    </div>
    {refs.length > 0 && <ul className="mem-refs" aria-label="References">
      {refs.map((ref, index) => <li key={`${index}:${ref.kind}:${ref.value}`} className="mem-ref"><span className="mem-ref-kind">{ref.kind}:</span>{ref.value}</li>)}
    </ul>}
    {standing.stale && <StaleNote stale={standing.stale} />}
  </div>;
}

function StaleNote({ stale }: { stale: aether.StaleNote }) {
  invariant(stale.reason.trim() !== "", "a stale mark says why");
  return <p className="mem-stale" role="note">
    <strong>Stale</strong> since {when(stale.since)}{stale.revision ? <> at <code className="b3-code">{stale.revision}</code></> : null} · {stale.reason}
    <span className="mem-stale-sub"> Retrieval ranks it at half score.</span>
  </p>;
}

/** "No support yet", or the supporting tasks and their independent runs. */
function supportText(support: aether.MemorySupport): string {
  if (support.tasks === 0) return "No support yet";
  return `${count(support.tasks, "task", "tasks")} · ${count(support.independent, "independent run", "independent runs")}`;
}
