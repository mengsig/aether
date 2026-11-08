import { Field } from "@aether-ui/ui";
import { useState } from "react";
import type { RunSummary } from "./runs.ts";

export function RunList({ runs, selected, onSelect }: {
  readonly runs: readonly RunSummary[];
  readonly selected: string | undefined;
  readonly onSelect: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const matching = runs.filter((run) => `${run.label} ${run.id}`.toLowerCase().includes(query.toLowerCase()));
  return (
    <aside className="topology-runs" aria-label="Runs">
      <Field label="Search runs" type="search" value={query} onChange={(event) => setQuery(event.currentTarget.value)} />
      {matching.length === 0 ? <p>No matching runs</p> : null}
      <ul className="topology-run-list">
        {matching.map((run) => (
          <li key={run.id}><button type="button" className="topology-run" aria-pressed={run.id === selected}
            onClick={() => onSelect(run.id)}>
            <span>{run.label}</span><small>{run.active ? "Active or pending" : "Finished"} · {run.count} {run.count === 1 ? "agent" : "agents"}</small>
          </button></li>
        ))}
      </ul>
    </aside>
  );
}
