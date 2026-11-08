import { routeHref } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { Badge, StatusPill } from "@aether-ui/ui";
import { duration, stagePill } from "../item/facts.ts";
import { usd } from "../queue/model.ts";
import { StageResult } from "./StageResult.tsx";
import "./pipeline.css";

export function modelLabel(model: archon.ModelRef): string {
  if (typeof model === "string") return model;
  const name = typeof model.model === "string" ? model.model : "Unknown model";
  return typeof model.provider === "string" ? `${model.provider} / ${name}` : name;
}

export function StageAttempt({ stage, now, conn, deadline }: { readonly stage: archon.StageRun; readonly now: number; readonly conn: string | null; readonly deadline?: string | null }) {
  const pill = stagePill(stage.state);
  const choice = stage.model_segments?.at(-1)?.choice ?? stage.choice;
  const error = "error" in stage.state ? stage.state.error : null;
  return <li className="item-stage" data-status={pill.status}>
    <div className="item-stage-head">
      <strong>{stage.stage_id}{stage.attempt > 1 ? ` · attempt ${stage.attempt}` : ""}{stage.fan_index > 0 ? ` · slot ${stage.fan_index + 1}` : ""}</strong>
      <StatusPill status={pill.status} label={pill.label} />
      {stage.verdict && stage.verdict !== "unknown" ? <Badge tone={stage.verdict === "pass" ? "success" : "danger"}>{stage.verdict}</Badge> : null}
    </div>
    <span className="item-muted">{[modelLabel(choice.model), choice.effort, usd(stage.cost_usd), duration(stage.started_at, stage.finished_at, now)].filter(Boolean).join(" · ")}</span>
    {deadline ? <span className="item-muted">Estimated deadline: <time dateTime={deadline}>{deadline}</time> (start + recorded limit)</span> : null}
    {error ? <p className="item-error">{error}</p> : null}
    {stage.summary ? <StageResult text={stage.summary} /> : null}
    {conn && stage.aether_agent ? <a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn, agent: stage.aether_agent })}>Open conversation</a> : null}
  </li>;
}
