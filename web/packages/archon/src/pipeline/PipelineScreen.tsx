// `#/archon/items/:item/runs/:run`: one run's pipeline from `run_plan`, each planned stage with its stage
// runs. A protocol 23 archond has no `run_plan`, so the view lists the stages the run has started instead.

import { routeHref, selectItem, selectRunPlan, useApp, useSelector } from "@aether-ui/core";
import type { archon, ConnectionInfo, RouteProps } from "@aether-ui/protocol";
import { Badge, EmptyState, EntityActionMenu, ErrorState, Skeleton, StatusPill } from "@aether-ui/ui";
import { useEffect, useMemo, useState } from "react";
import { useItemDetail } from "../item/detail.ts";
import { runPill } from "../item/facts.ts";
import { StageNode } from "../item/ItemSections.tsx";
import { useArchonAccess, useArchonConnection, type ArchonAccess } from "../queue/archon.ts";
import { invariant, usd } from "../queue/model.ts";
import { NotConnected, useNow } from "../queue/QueueScreen.tsx";
import { pipelineView, stageTerms, type PipelineStep, type PipelineView } from "./plan.ts";
import "../item/item.css";
import "./pipeline.css";

function Step({ step, index, fromPlan, now }: { readonly step: PipelineStep; readonly index: number; readonly fromPlan: boolean; readonly now: number }) {
  return (
    <li className="pipeline-step" data-current={step.current ? "true" : undefined} aria-current={step.current ? "step" : undefined}>
      <div className="pipeline-step-head">
        <span className="pipeline-index" aria-hidden="true">{index + 1}</span>
        <strong className="pipeline-name">{step.id}</strong>
        <StatusPill status={step.status} label={step.label} />
        {fromPlan && step.planned === null ? <Badge tone="warning">Not in the plan</Badge> : null}
      </div>
      {step.planned ? <p className="pipeline-terms">{stageTerms(step.planned).join(" · ")}</p> : null}
      {step.attempts.length > 0 ? (
        <ol className="item-list pipeline-attempts" aria-label={`Stage runs of ${step.id}`}>
          {step.attempts.map((stage) => <StageNode key={stage.id} stage={stage} now={now} />)}
        </ol>
      ) : null}
    </li>
  );
}

function PlanSource({ view, error }: { readonly view: PipelineView; readonly error: string | null }) {
  if (view.source === "plan") {
    return <p className="pipeline-note">Plan revision {view.revision} {view.edited ? <Badge tone="info">Edited after it started</Badge> : null}</p>;
  }
  const why = error ? `run_plan failed: ${error}.` : "This archond has no run_plan (protocol 23).";
  return <p className="pipeline-note" role="note">{why} Showing the stages this run has started, in order.</p>;
}

/** The run's plan from core's store, read once when missing; the item's rereads keep it current. */
function usePlan(access: ArchonAccess, item: string, run: string): { plan: archon.RunPlan | null; error: string | null; waiting: boolean } {
  const app = useApp();
  const served = access.has("run_plan");
  const stored = useSelector(useMemo(() => selectRunPlan(access.conn, run), [access.conn, run]));
  const [error, setError] = useState<{ run: string; text: string } | null>(null);
  const missing = stored === undefined;
  useEffect(() => {
    if (!served || !access.ready || !missing) return;
    let current = true;
    app.manager.readPlan(access.conn, item, run).then(
      () => { if (current) setError(null); },
      (failure: unknown) => { if (current) setError({ run, text: failure instanceof Error ? failure.message : String(failure) }); },
    );
    return () => { current = false; };
  }, [app, access.conn, access.ready, served, missing, item, run]);
  const plan = stored ? (stored.value as unknown as archon.RunPlan) : null;
  invariant(plan === null || stored?.item === item, `the plan of ${run} belongs to ${item}`);
  const failed = error?.run === run ? error.text : null;
  return { plan, error: failed, waiting: served && plan === null && failed === null };
}

function RunView({ access, detail, run, title }: { readonly access: ArchonAccess; readonly detail: archon.ItemDetail; readonly run: archon.PipelineRun; readonly title: string }) {
  const now = useNow();
  const { plan, error, waiting: waitingForPlan } = usePlan(access, detail.item.id, run.id);
  const view = useMemo(() => pipelineView(run, detail.stages, plan), [run, detail.stages, plan]);
  const pill = runPill(run.state);
  const number = detail.runs.findIndex((entry) => entry.id === run.id) + 1;
  invariant(number > 0, `run ${run.id} is one of the item's runs`);
  return (
    <article className="ui-page pipeline" aria-labelledby="pipeline-title">
      <header className="pipeline-header">
        <a className="ui-link ui-back item-back" href={routeHref("#/archon/items/:item", { item: detail.item.id })}>{title}</a>
        <h1 id="pipeline-title" className="ui-page-title item-title">Run {number} · {view.pipeline}</h1>
        <div className="pipeline-meta">
          <StatusPill status={pill.status} label={pill.label} />
          <span className="item-muted">{[run.branch, usd(run.cost_usd), run.id].join(" · ")}</span>
          <EntityActionMenu entity={{ kind: "run", conn: access.conn, id: run.id }} />
        </div>
        {waitingForPlan ? <Skeleton height="1.5rem" width="40%" /> : <PlanSource view={view} error={error} />}
      </header>
      {view.steps.length === 0 ? <EmptyState title="No stages yet">Stages appear once the run starts its first one. Open the item to check its waiting reasons.</EmptyState> : (
        <ol className="pipeline-steps" aria-label="Pipeline">
          {view.steps.map((step, index) => <Step key={step.id} step={step} index={index} fromPlan={view.source === "plan"} now={now} />)}
        </ol>
      )}
    </article>
  );
}

function Pipeline({ info, item, run }: { readonly info: ConnectionInfo; readonly item: string; readonly run: string }) {
  const access = useArchonAccess(info);
  const stored = useSelector(useMemo(() => selectItem(access.conn, item), [access.conn, item]));
  const read = useItemDetail(access, item);
  const detail = read.detail?.value as unknown as archon.ItemDetail | undefined;
  if (!detail) {
    if (read.error) return <ErrorState title="Could not load the run" message={read.error} onRetry={read.reload} />;
    return <div className="ui-page pipeline" aria-busy="true" aria-label="Loading the run"><Skeleton height="2rem" width="50%" /><Skeleton height="10rem" /></div>;
  }
  const found = detail.runs.find((entry) => entry.id === run);
  if (!found) return <EmptyState title="Run not found">Item {item} has no run {run}. <a className="ui-link" href={routeHref("#/archon/items/:item", { item })}>Back to the item</a></EmptyState>;
  const title = typeof stored?.summary.title === "string" ? stored.summary.title : detail.item.title;
  return <RunView access={access} detail={detail} run={found} title={title} />;
}

export default function PipelineRoute({ params }: RouteProps) {
  const info = useArchonConnection();
  const { item, run } = params;
  if (!item || !run) return <EmptyState title="No run named">Open a run from its item.</EmptyState>;
  return info ? <Pipeline info={info} item={item} run={run} /> : <NotConnected />;
}
