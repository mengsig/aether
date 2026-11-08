import { selectAgents, selectRead, useReread, useSelector } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Combo, Picker } from "@aether-ui/ui";
import { useState } from "react";
import { records } from "./shared.tsx";

export function useRepositories(conn: string): string[] {
  const agents = Object.values(useSelector(selectAgents(conn)));
  const workspaces = useReread(conn, "start:workspaces", { cmd: "list_workspaces" }, ["workspaces"]);
  const memories = useSelector(selectRead(conn, "memory:list"));
  const roots = [
    ...agents.map(a => a.scope.workspace_root),
    ...records<aether.Workspace>(workspaces?.value).map(w => w.path),
    ...records<aether.MemoryRecord>(memories?.value).flatMap(m => m.scope.scope === "repository" || m.scope.scope === "repository_role" ? [m.scope.id] : []),
  ];
  return [...new Set(roots.filter((root): root is string => typeof root === "string" && root !== ""))].sort();
}

export function Repository({ conn, label, value, onChange, testId, disabled, filter = false }: {
  conn: string; label: string; value: string; onChange: (value: string) => void; testId?: string; disabled?: boolean; filter?: boolean;
}) {
  const roots = useRepositories(conn);
  const [custom, setCustom] = useState(false);
  const suggestions = roots.map(id => ({ id, label: id }));
  if (filter && !custom && (value === "" || roots.includes(value))) return <Picker label={label} value={value} onChange={id => {
    if (id === "custom") setCustom(true); else onChange(id);
  }} options={[{ id: "", label: "All" }, ...suggestions, { id: "custom", label: "Another repository…" }]} />;
  return <Combo label={label} data-testid={testId} value={value} onChange={onChange} disabled={disabled}
    suggestions={suggestions.filter(s => s.id.toLowerCase().includes(value.toLowerCase()))} hint={filter ? "Blank for all repositories" : "Choose or enter a repository root"} />;
}

export function useScopeSuggestions(conn: string) {
  const roots = useRepositories(conn);
  const agents = Object.values(useSelector(selectAgents(conn)));
  const roles = [...new Set(agents.map(a => a.role).filter(Boolean))];
  return [{ id: "", label: "Every scope" }, { id: "global", label: "Global" },
    ...roots.map(root => ({ id: `repo:${root}`, label: `Repository ${root}` })),
    ...roles.map(role => ({ id: `role:${role}`, label: `Role ${role}` })),
    ...agents.flatMap(agent => [{ id: `agent:${agent.id}`, label: `Agent ${agent.name}` }, { id: `subtree:${agent.id}`, label: `${agent.name} and delegates` }]),
    ...roots.flatMap(root => roles.map(role => ({ id: `repo-role:${role}@${root}`, label: `${root} · ${role}` })))];
}
