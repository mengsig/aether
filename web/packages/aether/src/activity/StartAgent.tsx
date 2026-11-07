import { useState } from "react";
import { navigate, routeHref, selectAgents, selectRead, useApp, usePermission, useReread, useSelector } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Field, Sheet, Tabs, EmptyState } from "@aether-ui/ui";
import { Action, RunNote, StandingNotice, useRun, type Run } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";
import { ReadNote } from "../memory/shared.tsx";
import { configuredModels, createdAgent, DEFAULT_EFFORT, EFFORTS, modelGroups, modelKey, pickModel, recentWorkspaces, workspaceValidation } from "./data.ts";
import { SelectField, TextAreaField } from "./fields.tsx";
import { Workspace } from "./Workspace.tsx";
import "./activity.css";

const INACTIVE = ["completed", "failed", "cancelled"];

export function StartAgent({ conn, close }: { conn: string; close: () => void }) {
  const app = useApp();
  const run = useRun(conn);
  const [mode, setMode] = useState("agent");
  const [name, setName] = useState("");
  const [objective, setObjective] = useState("");
  const [picked, setPicked] = useState<string | null>(null);
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState(DEFAULT_EFFORT);
  const [owner, setOwner] = useState("");
  const agents = Object.values(useSelector(selectAgents(conn))).filter(a => !INACTIVE.includes(a.status.state));
  const models = useReread(conn, "start:models", { cmd: "list_models" }, ["models"]);
  const recent = useReread(conn, "start:workspaces", { cmd: "list_workspaces" }, ["workspaces"]);
  const choices = configuredModels(models?.value);
  const selected = pickModel(choices, model);
  const path = picked ?? recentWorkspaces(recent?.value)[0]?.path ?? "";
  const validation = workspaceValidation(useSelector(selectRead(conn, `start:validation:${path}`))?.value);
  const command = mode === "task" ? { cmd: "create_task", objective: objective.trim(), owner, priority: "normal" } :
    { cmd: "create_agent", name: name.trim() || null, role: "worker", objective: objective.trim(), start: true,
      scope: { workspace_root: validation?.path }, model: selected?.model, approval: { require_for: ["external"] }, config: { effort } };
  const disabled = usePermission(conn, command).refusal !== null;
  const blocked = blockedReason(mode, objective, agents.some(a => a.id === owner), selected, path, validation);
  const ownerName = agents.find(a => a.id === owner)?.name ?? owner;
  const submit = async () => {
    invariant(blocked === null, "a blocked form is not submitted");
    const reply = await run.send(command, "Starting…");
    if (reply === null) return;
    const started = mode === "task" ? { id: owner, message: `Started task for ${ownerName}` } : startedAgent(reply);
    close();
    app.store.report(started.message, conn, undefined, "success");
    navigate(routeHref("#/agents/:conn/:agent", { conn, agent: started.id }));
  };
  const footer = <div className="start-footer"><RunNote status={run.status} />
    <Action conn={conn} command={command} variant="primary" label={mode === "task" ? "Create task" : "Start agent"} blocked={blocked}
      busy={run.status.state === "busy" || run.status.state === "done"} onRun={submit} /></div>;
  return <Sheet open onClose={close} title="Start an agent" footer={footer}><div className="start-form"><StandingNotice conn={conn} />
    <Tabs label="Work type" selected={mode} onSelect={setMode} tabs={[{ id: "agent", label: "New agent", panel: null }, { id: "task", label: "Existing agent task", panel: null }]} />
    <TextAreaField label="What should the agent do?" value={objective} disabled={disabled} autoFocus onChange={setObjective} />
    {mode === "task" ? <SelectField label="Agent" value={owner} disabled={disabled} onChange={setOwner}>
      <option value="">Choose an agent</option>{agents.map(a => <option key={a.id} value={a.id}>{a.name}</option>)}</SelectField> : <>
      <Workspace conn={conn} path={path} change={setPicked} recent={recent} />
      <ModelChoice conn={conn} models={models} choices={choices} value={selected ? modelKey(selected) : ""} disabled={disabled} onChange={setModel} />
      <SelectField label="Effort" value={effort} disabled={disabled} onChange={setEffort}>
        {EFFORTS.map(e => <option key={e.value} value={e.value}>{e.label}</option>)}</SelectField>
      <Field label="Name" hint="Optional; the agent is named for you when blank" value={name} disabled={disabled} onChange={e => setName(e.target.value)} />
    </>}
  </div></Sheet>;
}

function startedAgent(reply: Awaited<ReturnType<Run["send"]>>): { id: string; message: string } {
  invariant(reply !== null, "a started agent has a reply");
  const agent = createdAgent(reply);
  return { id: agent.id, message: `Started ${agent.name}` };
}

function blockedReason(mode: string, objective: string, ownerActive: boolean, model: aether.ModelInfo | undefined, path: string,
  validation: aether.WorkspaceValidation | undefined): string | null {
  if (!objective.trim()) return "Describe the work to start";
  if (mode === "task") return ownerActive ? null : "Choose an active owner";
  if (!model) return "Choose a configured model";
  if (!path) return "Choose a workspace";
  if (!validation) return "Checking the workspace…";
  return validation.valid && validation.path?.startsWith("/") ? null : validation.reason ?? "Choose a valid workspace";
}

function ModelChoice({ conn, models, choices, value, disabled, onChange }: {
  conn: string; models: ReturnType<typeof useReread>; choices: readonly aether.ModelInfo[]; value: string; disabled: boolean; onChange: (key: string) => void;
}) {
  const app = useApp();
  invariant(value === "" || choices.some(m => modelKey(m) === value), "the shown model is one of the ready models");
  return <>
    <ReadNote entry={models} retry={() => app.reread(conn, "start:models", { cmd: "list_models" }, ["models"])} />
    {models?.issued && choices.length === 0 ? <EmptyState title="Connect a model provider on your host" /> :
      <SelectField label="Model" value={value} disabled={disabled || choices.length === 0} onChange={onChange}>
        {modelGroups(choices).map(g => <optgroup key={g.provider} label={g.provider}>
          {g.models.map(m => <option key={modelKey(m)} value={modelKey(m)}>{m.name}</option>)}</optgroup>)}</SelectField>}
  </>;
}
