import { useState } from "react";
import type { aether } from "@aether-ui/protocol";
import { Button, Field, Sheet, Tabs } from "@aether-ui/ui";
import { useApp, usePermission, useReread } from "@aether-ui/core";
import { Action, RunNote, useRun } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";
import { AgentPicker, records, ReadNote } from "./shared.tsx";
export const kinds = ["fact", "decision", "lesson", "constraint", "preference"];
export function CreateMemory({ conn, open, close, discard, refresh }: { conn: string; open: boolean; close: () => void; discard: () => void; refresh: () => Promise<unknown> }) {
  const [text, setText] = useState("");
  const [kind, setKind] = useState("fact");
  const [target, setTarget] = useState("global");
  const [repository, setRepository] = useState("");
  const [otherRepository, setOtherRepository] = useState(false);
  const [agent, setAgent] = useState("");
  const [tags, setTags] = useState("");
  const run = useRun(conn);
  const app = useApp();
  const workspaces = useReread(conn, "memory:workspaces", open ? { cmd: "list_workspaces" } : null, ["workspaces"]);
  const repositories = records<aether.Workspace>(workspaces?.value).filter(workspace => workspace.exists);
  const scope = createScope(target, repository.trim(), agent);
  const command = { cmd: "create_memory", spec: { text: text.trim(), kind, scope: scope ?? { scope: "global" }, tags: tags.split(",").map(t => t.trim()).filter(Boolean) } };
  const disabled = usePermission(conn, command).refusal !== null;
  return <Sheet open={open} onClose={close} title="Create memory"><div className="b3-stack">
    <Field label="Memory text" value={text} onChange={e => setText(e.target.value)} disabled={disabled} />
    <Tabs label="Memory kind" selected={kind} onSelect={setKind} tabs={kinds.map(id => ({ id, label: id, panel: null }))} />
    <Tabs label="Memory scope" selected={target} onSelect={setTarget} tabs={["global", "repository", "agent"].map(id => ({ id, label: id.charAt(0).toUpperCase() + id.slice(1), panel: null }))} />
    {target === "repository" && (repositories.length ? <><label className="b3-field"><span className="ui-field-label">Repository root</span><select className="b3-select" value={otherRepository ? "__other__" : repository} onChange={e => { setOtherRepository(e.target.value === "__other__"); setRepository(e.target.value === "__other__" ? "" : e.target.value); }} disabled={disabled}><option value="">Choose a repository</option>{repositories.map(workspace => <option key={workspace.path} value={workspace.path}>{workspace.path}</option>)}<option value="__other__">Other folder</option></select></label>{otherRepository && <Field label="Other repository root" value={repository} onChange={e => setRepository(e.target.value)} disabled={disabled} />}</> : <Field label="Repository root" value={repository} onChange={e => setRepository(e.target.value)} disabled={disabled} />)}
    {target === "repository" && workspaces?.error && <ReadNote entry={workspaces} retry={() => app.reread(conn, "memory:workspaces", { cmd: "list_workspaces" }, ["workspaces"])} />}
    {target === "agent" && <AgentPicker conn={conn} label="Agent" value={agent} onChange={setAgent} disabled={disabled} />}
    <Field label="Tags" hint="Comma separated" value={tags} onChange={e => setTags(e.target.value)} disabled={disabled} />
    <Action conn={conn} command={command} label="Save memory" blocked={text.trim() === "" ? "Enter memory text" : scope === null ? SCOPE_BLOCK[target] ?? null : null} busy={run.status.state === "busy"}
      onRun={async () => { if (await run.send(command)) { await refresh(); discard(); } }} />
    <RunNote status={run.status} />
    {run.status.state === "failed" && <Button variant="quiet" onClick={discard}>Discard</Button>}
  </div></Sheet>;
}

const SCOPE_BLOCK: Record<string, string> = { repository: "Choose a repository", agent: "Choose an agent" };

/** The scope to create in, or null while the chosen target lacks its repository or agent. */
function createScope(target: string, repository: string, agent: string): aether.OperatorMemoryScope | null {
  invariant(["global", "repository", "agent"].includes(target), "the scope target is one the form offers");
  invariant(repository === repository.trim(), "the repository id arrives trimmed");
  if (target === "global") return { scope: "global" };
  if (target === "repository") return repository === "" ? null : { scope: "repository", root: repository };
  return agent === "" ? null : { scope: "agent", id: agent };
}
