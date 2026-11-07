// Starting work: a chat (the default) is a task-less interactive assistant that waits for the next
// message after each reply, as the TUI starts one; a task agent finishes with its objective.

import { navigate, routeHref, selectAgents, selectRead, useApp, usePermission, useReread, useSelector } from "@aether-ui/core";
import { aether } from "@aether-ui/protocol";
import { Button, EmptyState, Field, Sheet, Tabs } from "@aether-ui/ui";
import { useState } from "react";
import { draftKey, writeDraft } from "../composer/drafts.ts";
import { Action, RunNote, StandingNotice, useRun, type Run } from "../controls/kit.tsx";
import { EFFORTS } from "../controls/commands.ts";
import { invariant } from "../fleet/invariant.ts";
import { ReadNote } from "../memory/shared.tsx";
import { configuredModels, workspaceValidation } from "./data.ts";
import { Workspace } from "./Workspace.tsx";

type Mode = "chat" | "agent" | "task";
const ENDED = new Set(["completed", "failed", "cancelled", "imported"]);
const MODE_HINTS: Readonly<Record<Mode, string>> = {
  chat: "A chat stays open: after each reply it waits for your next message until you end it.",
  agent: "A task agent works on its objective and ends, taking no more messages, once it is done.",
  task: "Give an agent that is still running one more task.",
};

export interface NewAgentDraft {
  readonly mode: "chat" | "agent";
  readonly name: string;
  readonly workspace: string;
  readonly model: aether.ModelRef;
  readonly effort: aether.Effort;
}

/** The create command for a chat or a task agent; only a task agent carries an objective. */
export function createCommand(draft: NewAgentDraft, objective: string): aether.CreateAgentCommand {
  invariant(draft.workspace.startsWith("/"), "an agent starts in a validated absolute workspace");
  invariant(objective.trim() !== "", "a new agent starts with a message or an objective");
  const common = { start: true, scope: { workspace_root: draft.workspace }, model: draft.model, approval: { require_for: ["external" as const] } };
  if (draft.mode === "agent") {
    return aether.createAgent({ ...common, name: draft.name.trim() || null, role: "worker", objective: objective.trim(), config: { effort: draft.effort } });
  }
  return aether.createAgent({ ...common, name: draft.name.trim() || "assistant", role: "assistant", config: { effort: draft.effort, interactive: true } });
}

/**
 * A chat gets its first message once it exists, then opens. The message waits as the chat's draft until
 * it is delivered, so a refused or unconfirmed send is retried from the chat, never by a second create.
 */
async function startChat(run: Run, conn: string, command: aether.CreateAgentCommand, message: string, close: () => void): Promise<void> {
  invariant(command.objective === undefined, "a chat is started without a task");
  invariant(message.trim() !== "", "a chat starts with a message");
  const reply = await run.send(command, "Started");
  if (reply === null) return;
  const agent = reply.data as { id?: unknown } | null;
  invariant(reply.reply === "agent" && typeof agent?.id === "string", "create_agent answers with the agent it made");
  const draft = draftKey(conn, agent.id, "message");
  writeDraft(draft, message.trim());
  if (await run.send(aether.sendMessage({ to: agent.id, content: message.trim() }), "Sent")) writeDraft(draft, "");
  close();
  navigate(routeHref("#/agents/:conn/:agent", { conn, agent: agent.id }));
}

function useStartData(conn: string, path: string) {
  const app = useApp();
  const live = Object.values(useSelector(selectAgents(conn))).filter(a => !ENDED.has(a.status.state));
  const models = useReread(conn, "start:models", { cmd: "list_models" }, ["models"]);
  const validation = workspaceValidation(useSelector(selectRead(conn, `start:validation:${path}`))?.value);
  const retryModels = () => app.reread(conn, "start:models", { cmd: "list_models" }, ["models"]);
  return { live, models, choices: configuredModels(models?.value), validation, retryModels };
}

export function StartAgent({ conn, close }: { conn: string; close: () => void }) {
  const run = useRun(conn);
  const [mode, setMode] = useState<Mode>("chat");
  const [name, setName] = useState("");
  const [objective, setObjective] = useState("");
  const [path, setPath] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState<aether.Effort>("medium");
  const [owner, setOwner] = useState("");
  const { live, models, choices, validation, retryModels } = useStartData(conn, path);
  const selected = choices.find(m => `${m.model.provider}/${m.model.model}` === model) ?? choices.find(m => m.is_default) ?? choices[0];
  const workspace = validation?.valid ? validation.path ?? "" : "";
  const command = mode === "task" ? aether.createTask({ objective: objective.trim(), owner, priority: "normal" })
    : selected && workspace !== "" && objective.trim() !== "" ? createCommand({ mode, name, workspace, model: selected.model, effort }, objective) : aether.createAgent({ role: "assistant" });
  const disabled = usePermission(conn, command).refusal !== null;
  const blocked = !objective.trim() ? (mode === "chat" ? "Write the first message" : "Describe the work to start") : mode === "task" ?
    (live.some(a => a.id === owner) ? null : "Choose an active owner") : !selected ? "Choose a configured model" :
      workspace.startsWith("/") ? null : "Validate the workspace first";
  const start = () => (command.cmd === "create_agent" && mode === "chat" ? startChat(run, conn, command, objective, close) : run.send(command));
  return <Sheet open onClose={close} title="Start an agent"><div className="b3-stack"><StandingNotice conn={conn} />
    <Tabs label="Work type" selected={mode} onSelect={id => setMode(id as Mode)} tabs={[{ id: "chat", label: "Chat", panel: null }, { id: "agent", label: "Task agent", panel: null }, { id: "task", label: "Existing agent task", panel: null }]} />
    <p className="b3-section-hint">{MODE_HINTS[mode]}</p>
    <Field label={mode === "chat" ? "First message" : "Objective"} value={objective} disabled={disabled} onChange={e => setObjective(e.target.value)} />
    {mode === "task" ? <div className="b3-row">{live.map(a => <Button key={a.id} disabled={disabled} aria-pressed={owner === a.id} onClick={() => setOwner(a.id)}>{a.name}</Button>)}</div> : <>
      <Field label="Name" value={name} disabled={disabled} onChange={e => setName(e.target.value)} />
      <Workspace conn={conn} path={path} change={setPath} />
      <ReadNote entry={models} retry={retryModels} />
      <div className="b3-row">{choices.map(m => <Button key={`${m.model.provider}/${m.model.model}`} disabled={disabled} aria-pressed={selected === m} onClick={() => setModel(`${m.model.provider}/${m.model.model}`)}>{m.name}</Button>)}</div>
      {models?.issued && choices.length === 0 ? <EmptyState title="Connect a model provider on your host" /> : null}
      <div className="b3-row" aria-label="Effort">{EFFORTS.map(value => <Button key={value} disabled={disabled} aria-pressed={effort === value} onClick={() => setEffort(value)}>{value}</Button>)}</div>
    </>}
    <Action conn={conn} command={command} label={mode === "task" ? "Create task" : mode === "chat" ? "Start chat" : "Start agent"} blocked={blocked} busy={run.status.state === "busy"} onRun={start} />
    <RunNote status={run.status} />
  </div></Sheet>;
}
