import { useState } from "react";
import { selectAgents, selectRead, useApp, usePermission, useReread, useSelector } from "@aether-ui/core";
import { Field, Sheet, Tabs, Button, EmptyState } from "@aether-ui/ui";
import { Action, RunNote, StandingNotice, useRun } from "../controls/kit.tsx";
import { ReadNote } from "../memory/shared.tsx";
import { configuredModels, workspaceValidation } from "./data.ts";
import { Workspace } from "./Workspace.tsx";

export function StartAgent({ conn, close }: { conn: string; close: () => void }) {
  const app = useApp();
  const run = useRun(conn);
  const [mode, setMode] = useState("agent");
  const [name, setName] = useState("");
  const [objective, setObjective] = useState("");
  const [path, setPath] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("medium");
  const [owner, setOwner] = useState("");
  const agents = Object.values(useSelector(selectAgents(conn))).filter(a => !["completed", "failed", "cancelled"].includes(a.status.state));
  const models = useReread(conn, "start:models", { cmd: "list_models" }, ["models"]);
  const choices = configuredModels(models?.value);
  const selected = choices.find(m => `${m.model.provider}/${m.model.model}` === model) ?? choices.find(m => m.is_default) ?? choices[0];
  const validation = workspaceValidation(useSelector(selectRead(conn, `start:validation:${path}`))?.value);
  const command = mode === "task" ? { cmd: "create_task", objective: objective.trim(), owner, priority: "normal" } :
    { cmd: "create_agent", name: name.trim() || null, role: "worker", objective: objective.trim(), start: true,
      scope: { workspace_root: validation?.path }, model: selected?.model, approval: { require_for: ["external"] }, config: { effort } };
  const disabled = usePermission(conn, command).refusal !== null;
  const blocked = !objective.trim() ? "Describe the work to start" : mode === "task" ?
    (agents.some(a => a.id === owner) ? null : "Choose an active owner") : !selected ? "Choose a configured model" :
      validation?.valid && validation.path?.startsWith("/") ? null : "Validate the workspace first";
  return <Sheet open onClose={close} title="Start an agent"><div className="b3-stack"><StandingNotice conn={conn} />
    <Tabs label="Work type" selected={mode} onSelect={setMode} tabs={[{ id: "agent", label: "New agent", panel: null }, { id: "task", label: "Existing agent task", panel: null }]} />
    <Field label="Objective" value={objective} disabled={disabled} onChange={e => setObjective(e.target.value)} />
    {mode === "task" ? <div className="b3-row">{agents.map(a => <Button key={a.id} disabled={disabled} aria-pressed={owner === a.id} onClick={() => setOwner(a.id)}>{a.name}</Button>)}</div> : <>
      <Field label="Name" value={name} disabled={disabled} onChange={e => setName(e.target.value)} />
      <Workspace conn={conn} path={path} change={setPath} />
      <ReadNote entry={models} retry={() => app.reread(conn, "start:models", { cmd: "list_models" }, ["models"])} />
      <div className="b3-row">{choices.map(m => <Button key={`${m.model.provider}/${m.model.model}`} disabled={disabled} aria-pressed={selected === m} onClick={() => setModel(`${m.model.provider}/${m.model.model}`)}>{m.name}</Button>)}</div>
      {models?.issued && choices.length === 0 ? <EmptyState title="Connect a model provider on your host" /> : null}
      <div className="b3-row" aria-label="Effort">{["low", "medium", "high", "x_high", "max"].map(value => <Button key={value} disabled={disabled} aria-pressed={effort === value} onClick={() => setEffort(value)}>{value}</Button>)}</div>
    </>}
    <Action conn={conn} command={command} label={mode === "task" ? "Create task" : "Start agent"} blocked={blocked} busy={run.status.state === "busy"} onRun={() => run.send(command)} />
    <RunNote status={run.status} />
  </div></Sheet>;
}
