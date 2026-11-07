import { useState } from "react";
import { connectionDraftKey, navigate, routeHref, selectAgents, selectRead, useDraft, usePermission, useSelector } from "@aether-ui/core";
import { Field, Sheet, Segmented, Button, Picker, TextArea } from "@aether-ui/ui";
import { Action, RunNote, StandingNotice, useRun } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";
import { configuredModels, defaultModel, workspaceValidation } from "./data.ts";
import { Models } from "./Models.tsx";
import { Workspace } from "./Workspace.tsx";

const INITIAL = { mode: "agent", name: "", objective: "", path: "", model: "", effort: "medium", owner: "", priority: "normal", role: "worker", approval: "external" };
type StartDraft = typeof INITIAL;
const options = (values: string[]) => values.map(id => ({ id, label: id.replaceAll("_", " ") }));

export function StartAgent({ conn, close }: { conn: string; close: () => void }) {
  const run = useRun(conn);
  const draft = useDraft(connectionDraftKey("activity", conn, "start-agent"), INITIAL);
  const d = draft.value;
  const agents = Object.values(useSelector(selectAgents(conn))).filter(a => !["completed", "failed", "cancelled"].includes(a.status.state));
  const models = configuredModels(useSelector(selectRead(conn, "start:models"))?.value);
  const ref = d.model || defaultModel(models);
  const selected = models.find(m => `${m.model.provider}/${m.model.model}` === ref && m.configured);
  const validationRead = useSelector(selectRead(conn, `start:validation:${d.path}`));
  const validation = workspaceValidation(validationRead?.error ? undefined : validationRead?.value);
  const command = d.mode === "task" ? { cmd: "create_task", objective: d.objective.trim(), owner: d.owner, priority: d.priority } :
    { cmd: "create_agent", name: d.name.trim() || null, role: d.role, objective: d.objective.trim(), start: true,
      scope: { workspace_root: validation?.path }, model: selected?.model, approval: { require_for: [d.approval] }, config: { effort: d.effort } };
  const disabled = usePermission(conn, command).refusal !== null;
  const blocked = !d.objective.trim() ? "Describe the work to start" : d.mode === "task" ?
    (agents.some(a => a.id === d.owner && !a.current_task) ? null : "Choose an agent that has no current task") : !selected ? "Choose a configured model" :
      validation?.valid ? null : "Choose a valid workspace";
  const submit = async () => {
    const reply = await run.send(command);
    if (!reply) return;
    const agent = d.mode === "task" ? d.owner : (reply.data as { id?: string } | undefined)?.id;
    invariant(typeof agent === "string" && agent.length > 0, "successful start identifies the agent");
    invariant(d.mode === "task" || reply.reply === "agent", "create_agent returns an agent");
    draft.clearIfUnchanged(); close(); navigate(routeHref("#/agents/:conn/:agent", { conn, agent }));
  };
  return <Sheet open onClose={close} title="Start an agent"><div className="b3-stack"><StandingNotice conn={conn} />
    <div data-testid="start-mode"><Segmented label="Work type" value={d.mode} onChange={mode => draft.update({ mode })}
      options={[{ id: "agent", label: "New agent" }, { id: "task", label: "Task for an existing agent" }]} /></div>
    <TextArea label="Objective" data-testid="start-objective" data-autofocus rows={3} value={d.objective} disabled={disabled} onChange={e => draft.update({ objective: e.target.value })} />
    {d.mode === "task" ? <><Picker label="Owner" data-testid="start-owner" value={d.owner} disabled={disabled} onChange={owner => draft.update({ owner })}
      options={agents.map(a => ({ id: a.id, label: a.name, group: a.status.state, ...ownerAvailability(a) }))} />
      <Segmented label="Priority" value={d.priority} onChange={priority => draft.update({ priority })} options={options(["low", "normal", "high", "critical"])} /></> :
      <AgentFields conn={conn} draft={d} edit={draft.update} model={ref} disabled={disabled} />}
    <Action conn={conn} command={command} label={d.mode === "task" ? "Create task" : "Start agent"} data-testid="start-submit" variant="primary"
      blocked={blocked} busy={run.status.state === "busy"} onRun={submit} />
    <RunNote status={run.status} />
  </div></Sheet>;
}

/** An agent carries one task at a time, so one that holds a task is listed but cannot be chosen. */
function ownerAvailability(agent: { id: string; current_task?: string | null }) {
  return agent.current_task ? { description: "Already working on a task", disabled: true } : { description: agent.id };
}

function AgentFields({ conn, draft: d, edit, model, disabled }: { conn: string; draft: StartDraft; edit: (patch: Partial<StartDraft>) => void; model: string; disabled: boolean }) {
  const agents = Object.values(useSelector(selectAgents(conn)));
  const roles = [...new Set(["worker", "lead", "reviewer", ...agents.map(agent => agent.role).filter(Boolean)])];
  return <>
    <Field label="Name" hint="Optional" data-testid="start-name" value={d.name} disabled={disabled} onChange={e => edit({ name: e.target.value })} />
    <Workspace conn={conn} path={d.path} change={path => edit({ path })} defaults />
    <Models conn={conn} screen="start" value={model} onChange={model => edit({ model })} disabled={disabled} testId="start-model" />
    <div data-testid="start-effort"><Segmented label="Effort" value={d.effort} disabled={disabled} onChange={effort => edit({ effort })} options={options(["low", "medium", "high", "x_high", "max"])} /></div>
    <details><summary>Advanced</summary><div className="b3-stack">
      <Picker label="Approval policy" disabled={disabled} value={d.approval} onChange={approval => edit({ approval })} options={options(["external", "mutation", "idempotent_mutation", "read_only"])} />
      <Picker label="Role" disabled={disabled} value={d.role} onChange={role => edit({ role })} options={options(roles)} />
    </div></details>
  </>;
}

export function StartAgentButton({ conn, label = "Start an agent" }: { conn: string; label?: string }) {
  const [open, setOpen] = useState(false);
  return <><Button variant="primary" onClick={() => setOpen(true)}>{label}</Button>{open ? <StartAgent conn={conn} close={() => setOpen(false)} /> : null}</>;
}
