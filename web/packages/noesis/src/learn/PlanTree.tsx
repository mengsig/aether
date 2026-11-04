// The session's plan: its steps in prerequisite order, each with a word for where the learner is.

import type { aether } from "@aether-ui/protocol";
import { useMemo } from "react";
import { PLAN_STATUS_LABEL, planRows } from "./model.ts";

export function PlanTree({ plan }: { plan: aether.PlanTree }) {
  const rows = useMemo(() => planRows(plan), [plan]);
  const done = rows.filter((row) => row.node.status === "visited").length;
  return (
    <section className="learn-plan" aria-label="Plan">
      <h2 className="learn-subtitle">Your path <span className="learn-muted">{done} of {rows.length} done</span></h2>
      <ol className="learn-plan-list">
        {rows.map(({ node, depth }) => (
          <li key={node.id} className="learn-plan-step" data-status={node.status} style={{ marginInlineStart: `calc(${Math.min(depth, 4)} * var(--space-4))` }}
            aria-current={node.status === "current" ? "step" : undefined}>
            <span className="learn-plan-dot" aria-hidden="true" />
            <span className="learn-plan-title">{node.title}</span>
            <span className="learn-plan-status">{PLAN_STATUS_LABEL[node.status]}</span>
          </li>
        ))}
      </ol>
    </section>
  );
}
