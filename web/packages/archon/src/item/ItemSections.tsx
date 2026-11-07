// The item page's sections: details, runs, the stage timeline, gates, GitHub links and reports.

import { routeHref } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { Badge, EmptyState, StatusPill } from "@aether-ui/ui";
import type { ReactNode } from "react";
import { dateTime, invariant, priorityLabel, safeHttpUrl, usd } from "../queue/model.ts";
import { duration, modelOf, objectFor, runPill, stagePill } from "./facts.ts";

export function Section({ title, children }: { readonly title: string; readonly children: ReactNode }) {
  return <section className="ui-card item-section" aria-label={title}><h2 className="ui-card-title item-heading">{title}</h2>{children}</section>;
}

function Row({ label, children }: { readonly label: string; readonly children: ReactNode }) {
  return <div className="item-fact-row"><dt>{label}</dt><dd>{children}</dd></div>;
}

const REVIEW_LABEL: Readonly<Record<string, string>> = { strict: "Strict", standard: "Standard", none: "None" };

export function Details({ detail, repoName, pipeline, prUrl }: {
  readonly detail: archon.ItemDetail; readonly repoName: string | null; readonly pipeline: string; readonly prUrl: string | null;
}) {
  const item = detail.item;
  const pr = safeHttpUrl(prUrl ?? "");
  return (
    <dl className="item-facts-list">
      <Row label="Repository">{repoName ?? item.repo ?? "No repository"}</Row>
      {pr ? <Row label="Pull request"><a className="ui-link" href={pr} target="_blank" rel="noopener noreferrer">Open</a></Row> : null}
      <Row label="Pipeline">{pipeline}{pipeline !== item.pipeline ? ` (queued as ${item.pipeline})` : ""}</Row>
      <Row label="Priority">{priorityLabel(item.priority)}</Row>
      <Row label="Review">{REVIEW_LABEL[item.autonomy?.review ?? ""] ?? "Default"}</Row>
      <Row label="Total cost">{usd(detail.spent_usd)}</Row>
      {item.budget_usd ? <Row label="Budget">{usd(item.budget_usd)}</Row> : null}
      <Row label="Created">{dateTime(item.created_at)}</Row>
      <Row label="Updated">{dateTime(item.updated_at)}</Row>
      {item.dependencies.length > 0 ? (
        <Row label="Depends on">
          {item.dependencies.map((dep) => <a key={dep} className="ui-link item-dep" href={routeHref("#/archon/items/:item", { item: dep })}>{dep}</a>)}
        </Row>
      ) : null}
    </dl>
  );
}

export function Runs({ item, runs, now }: { readonly item: string; readonly runs: readonly archon.PipelineRun[]; readonly now: number }) {
  if (runs.length === 0) return <EmptyState title="No runs yet">A run starts when Archon dispatches the item.</EmptyState>;
  return (
    <ol className="item-list">
      {runs.map((run, index) => {
        const pill = runPill(run.state);
        const took = duration(run.started_at, run.finished_at, now);
        return (
          <li key={run.id} className="item-run">
            <a className="ui-link" href={routeHref("#/archon/items/:item/runs/:run", { item, run: run.id })}>Run {index + 1} · {run.pipeline}</a>
            <StatusPill status={pill.status} label={pill.label} />
            <span className="item-muted">{[dateTime(run.started_at), took, usd(run.cost_usd)].filter(Boolean).join(" · ")}</span>
          </li>
        );
      })}
    </ol>
  );
}

const VERDICT_TONE: Readonly<Record<archon.Verdict, "success" | "danger" | "neutral">> = { pass: "success", fail: "danger", unknown: "neutral" };

export function StageNode({ stage, now }: { readonly stage: archon.StageRun; readonly now: number }) {
  const pill = stagePill(stage.state);
  const verdict = stage.verdict ?? null;
  const name = [stage.stage_id, stage.attempt > 1 ? `attempt ${stage.attempt}` : null, stage.fan_index > 0 ? `slot ${stage.fan_index + 1}` : null].filter(Boolean).join(" · ");
  const error = "error" in stage.state && typeof stage.state.error === "string" ? stage.state.error : null;
  return (
    <li className="item-stage" data-status={pill.status}>
      <div className="item-stage-head">
        <strong>{name}</strong>
        <StatusPill status={pill.status} label={pill.label} />
        {verdict ? <Badge tone={VERDICT_TONE[verdict] ?? "neutral"}>{verdict}</Badge> : null}
      </div>
      <span className="item-muted">{[modelOf(stage), usd(stage.cost_usd), duration(stage.started_at, stage.finished_at, now)].filter(Boolean).join(" · ")}</span>
      {error ? <p className="item-error">{error}</p> : null}
      {stage.summary ? <p className="item-summary">{stage.summary}</p> : null}
    </li>
  );
}

export function Stages({ detail, now }: { readonly detail: archon.ItemDetail; readonly now: number }) {
  if (detail.stages.length === 0) return <EmptyState title="No stages yet">Stages appear here once the item is dispatched.</EmptyState>;
  const numbers = new Map(detail.runs.map((run, index) => [run.id, index + 1]));
  const groups: { run: string; stages: archon.StageRun[] }[] = [];
  for (const stage of detail.stages) {
    const last = groups.at(-1);
    if (last?.run === stage.run) last.stages.push(stage);
    else groups.push({ run: stage.run, stages: [stage] });
  }
  invariant(groups.reduce((sum, group) => sum + group.stages.length, 0) === detail.stages.length, "every stage sits in one run group");
  return (
    <div className="item-timeline">
      {groups.map((group, index) => (
        <div key={`${group.run}-${index}`}>
          {numbers.size > 1 ? <h3 className="item-subheading">Run {numbers.get(group.run) ?? "?"}</h3> : null}
          <ol className="item-list" aria-label="Stages">{group.stages.map((stage) => <StageNode key={stage.id} stage={stage} now={now} />)}</ol>
        </div>
      ))}
    </div>
  );
}

const GATE_LOOK: Readonly<Record<string, { tone: "success" | "danger" | "warning" | "neutral"; label: string }>> = {
  approve: { tone: "success", label: "Approved" }, revise: { tone: "warning", label: "Revise" }, reject: { tone: "danger", label: "Rejected" },
  withdrawn: { tone: "neutral", label: "Withdrawn" }, waiting: { tone: "warning", label: "Waiting on you" },
};

export function Gates({ gates }: { readonly gates: readonly archon.OperatorGate[] }) {
  return (
    <ul className="item-list">
      {gates.map((gate) => {
        const decision = gate.decision?.decision ?? (gate.withdrawn ? "withdrawn" : "waiting");
        const look = GATE_LOOK[decision] ?? { tone: "neutral", label: decision };
        return (
          <li key={`${gate.run}-${gate.stage_id}-${gate.attempt}`} className="item-gate">
            <div className="item-gate-content"><strong>{gate.subject === "proposal" ? "Proposal" : "Report"} · {gate.stage_id}</strong>
              {gate.summary.length > 180 ? <details><summary>{gate.summary.slice(0, 180)}…</summary><p>{gate.summary}</p></details> : <p>{gate.summary}</p>}
            </div>
            <Badge tone={look.tone}>{look.label}</Badge>
          </li>
        );
      })}
    </ul>
  );
}

const githubUrl = (ref: archon.GithubRef): string | null =>
  safeHttpUrl(`https://${ref.host}/${ref.repo}/${ref.kind === "issue" ? "issues" : "pull"}/${ref.number}`);

/** The link's page in the GitHub screens (CONTRACTS.md §2.3); null for a repository not named `owner/name`. */
function pageHref(ref: archon.GithubRef): string | null {
  const [owner, repo, ...rest] = ref.repo.split("/");
  if (!owner || !repo || rest.length > 0) return null;
  return routeHref("#/github/:host/:owner/:repo/:number", { host: ref.host, owner, repo, number: String(ref.number) });
}

/** Each link with what GitHub last said of it; its text is untrusted, so it is shown, never followed blindly. */
export function Links({ links, objects }: { readonly links: readonly archon.GithubLink[]; readonly objects: readonly archon.GithubObject[] }) {
  if (links.length === 0) return <EmptyState title="No GitHub links">Issues and pull requests linked to this item show up here.</EmptyState>;
  return (
    <ul className="item-list">
      {links.map((link) => {
        const object = objectFor(link.ref, objects);
        const url = githubUrl(link.ref);
        const label = `${link.ref.repo}#${link.ref.number}`;
        const page = pageHref(link.ref);
        return (
          <li key={`${label}-${link.relation}`} className="item-link">
            {url ? <a className="ui-link" href={url} target="_blank" rel="noopener noreferrer">{label}</a> : <span>{label}</span>}
            <Badge tone="neutral">{link.ref.kind === "issue" ? "Issue" : "Pull request"} · {link.relation}</Badge>
            {object ? <span className="item-muted">{object.title} · {object.state}</span> : <span className="item-muted">not read from GitHub yet</span>}
            {page ? <a className="ui-link" href={page} aria-label={`Open the page of ${label}`}>Open page</a> : null}
          </li>
        );
      })}
    </ul>
  );
}

export function Reports({ reports }: { readonly reports: readonly archon.ReportSummary[] }) {
  return (
    <ul className="item-list">
      {reports.map((report) => {
        const url = safeHttpUrl(report.location);
        return (
          <li key={report.report} className="item-link">
            <span>{report.title}</span>
            {url ? <a className="ui-link" href={url} target="_blank" rel="noopener noreferrer">Open</a> : <Badge tone="neutral">{report.state}</Badge>}
          </li>
        );
      })}
    </ul>
  );
}
