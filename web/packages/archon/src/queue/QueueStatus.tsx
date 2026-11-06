// The queue's header: the global pause, what runs against its caps, spend against budgets, open questions,
// and the two ways to give Archon work: a new item, or a message to the orchestrator.

import type { ArchonItem } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { Badge, Button, ErrorState, Menu } from "@aether-ui/ui";
import { useMemo, useState } from "react";
import { useChange, useRead, type ArchonAccess } from "./archon.ts";
import { usd } from "./model.ts";
import { QuestionList } from "./Questions.tsx";

const count = (n: number, noun: string): string => `${n} ${noun}${n === 1 ? "" : "s"}`;

function Meter({ label, value, cap, format }: { readonly label: string; readonly value: number; readonly cap: number; readonly format: (n: number) => string }) {
  return (
    <div className="queue-meter">
      <span className="queue-meter-label">{label}</span>
      <span className="queue-meter-value">{cap > 0 ? `${format(value)} of ${format(cap)}` : `${format(value)} · no limit`}</span>
      {cap > 0 ? <meter min={0} max={cap} value={Math.min(value, cap)} aria-label={label} /> : null}
    </div>
  );
}

function PauseControl({ access, status, onDone }: { readonly access: ArchonAccess; readonly status: archon.DaemonStatus; readonly onDone: () => void }) {
  const change = useChange(access);
  const [error, setError] = useState<string | null>(null);
  const paused = status.pause.global;
  const toggle = async () => {
    // Soft only: a device's hard pause is refused by archond, and core refuses it first.
    const result = await change({ cmd: paused ? "resume" : "pause", scope: "global", hard: false });
    setError(result.ok ? null : result.error);
    onDone();
  };
  return (
    <div className="queue-pause" data-paused={paused ? "true" : undefined}>
      <strong>{paused ? "Paused" : "Running"}</strong>
      <span className="queue-muted">{paused ? "Nothing new starts; running agents finish their stages." : `${status.queued_items} queued · ${status.running_agents} agents running`}</span>
      <Button variant={paused ? "primary" : "secondary"} size="sm" disabled={access.refusal !== null} title={access.refusal ?? undefined} onClick={() => void toggle()}>
        {paused ? "Resume new work" : "Pause new work"}
      </Button>
      {error ? <span role="alert" className="queue-error">{error}</span> : null}
    </div>
  );
}

function Spend({ access, status, names, settled }: {
  readonly access: ArchonAccess; readonly status: archon.DaemonStatus; readonly names: ReadonlyMap<string, string>; readonly settled: string;
}) {
  const cost = useRead<archon.CostReport>(access, { cmd: "cost_report", since: null, until: null, by: "repo" }, ["cost_report"], settled);
  const caps = status.caps;
  return (
    <div className="queue-spend-box" aria-label="Spend">
      <Meter label="Spend today" value={status.spend_today_usd} cap={caps.budgets_enabled ? caps.budget_usd_daily ?? 0 : 0} format={usd} />
      <Meter label="Spend in total" value={status.spend_total_usd} cap={caps.budgets_enabled ? caps.budget_usd_total ?? 0 : 0} format={usd} />
      {cost.data ? (
        <p className="queue-muted">
          {cost.data.range.since} to {cost.data.range.until}: {usd(cost.data.total.counted_usd)}
          {cost.data.rows.slice(0, 4).map((row) => ` · ${row.key ? names.get(row.key) ?? row.key : "other"} ${usd(row.counted_usd)}`).join("")}
        </p>
      ) : null}
    </div>
  );
}

/** `settled` is archond's activity as of its last settle: status, counts and questions are reread when it moves. */
export function QueueStatus({ access, items, names, settled, onDialog }: {
  readonly access: ArchonAccess; readonly onDialog: (dialog: "item" | "say" | "repo") => void; readonly items: Readonly<Record<string, ArchonItem>>; readonly names: ReadonlyMap<string, string>; readonly settled: string;
}) {
  const status = useRead<archon.DaemonStatus>(access, { cmd: "status" }, ["status"], settled);
  const questions = useRead<archon.Question[]>(access, { cmd: "list_questions", open_only: false }, ["questions"], settled);
  const open = (questions.data ?? []).filter((question) => !question.answer);
  const titles = useMemo(() => new Map(Object.entries(items).map(([id, item]) => [id, String(item.summary.title)])), [items]);
  return (
    <div className="queue-status">
      <div className="queue-actions">
        <Button variant="primary" disabled={access.refusal !== null} title={access.refusal ?? undefined} onClick={() => onDialog("item")}>New item</Button>
        <Button disabled={access.refusal !== null} title={access.refusal ?? undefined} onClick={() => onDialog("say")}>Message Archon</Button>
        {access.refusal === null ? <Menu label="More" items={[{ id: "register-repository", label: "Register a repository", onSelect: () => onDialog("repo") }]} /> : null}
      </div>
      {status.error && !status.data ? <ErrorState title="Could not read Archon's status" message={status.error} onRetry={status.reload} /> : null}
      {status.data ? (
        <div className="ui-card queue-summary">
          <PauseControl access={access} status={status.data} onDone={status.reload} />
          <div className="queue-counts">
            <Meter label="Agents" value={status.data.running_agents} cap={status.data.caps.max_parallel_agents ?? 0} format={String} />
            <span><Badge tone={status.data.blocked_items ? "warning" : "neutral"}>{count(status.data.blocked_items, "blocked item")}</Badge></span>
            <span><Badge tone={status.data.open_questions ? "warning" : "neutral"}>{count(status.data.open_questions, "open question")}</Badge></span>
            <span><Badge tone={status.data.open_gates ? "warning" : "neutral"}>{count(status.data.open_gates, "open gate")}</Badge></span>
          </div>
          <Spend access={access} status={status.data} names={names} settled={settled} />
        </div>
      ) : null}
      {open.length > 0 ? <QuestionList access={access} questions={open} titles={titles} onAnswered={questions.reload} /> : null}
    </div>
  );
}
