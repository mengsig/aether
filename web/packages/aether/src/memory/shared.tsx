import { readInFlight, selectAgents, useSelector, type ReadEntry } from "@aether-ui/core";
import { useMemo } from "react";
import { ErrorState, Skeleton, Picker } from "@aether-ui/ui";
import { invariant } from "../fleet/invariant.ts";

export function records<T>(value: unknown): T[] {
  if (value === null || value === undefined) return [];
  invariant(Array.isArray(value), "the daemon returned a list");
  invariant(value.every(item => item !== null && typeof item === "object"), "list entries are records");
  return value as T[];
}

export function ReadNote({ entry, retry }: { entry: ReadEntry | undefined; retry: () => unknown }) {
  if (entry?.error) return <ErrorState message={entry.error} onRetry={() => { void retry(); }} />;
  if (!entry || readInFlight(entry)) return <div aria-busy="true"><Skeleton /></div>;
  return null;
}

/** Picks one of this connection's agents by name; the value is the agent id, "" when none is chosen. */
export function AgentPicker({ conn, label, value, onChange, disabled = false }: {
  conn: string; label: string; value: string; onChange: (id: string) => void; disabled?: boolean;
}) {
  const agents = useSelector(useMemo(() => selectAgents(conn), [conn]));
  const list = Object.values(agents);
  invariant(typeof value === "string", "an agent picker holds an id");
  invariant(list.every(agent => agent.id.length > 0), "every agent has an id");
  return <Picker label={label} value={value} disabled={disabled} onChange={onChange}
    options={[{ id: "", label: "Choose an agent" }, ...list.map(agent => ({ id: agent.id, label: agent.name || agent.id, description: agent.id }))]} />;
}
