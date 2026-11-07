// Starting work: a chat (the default) is a task-less interactive assistant that waits for the next
// message after each reply, as the TUI starts one; a task agent finishes with its objective.

import { navigate, routeHref, selectAgents, selectRead, useApp, usePermission, useReread, useSelector } from "@aether-ui/core";
import { aether } from "@aether-ui/protocol";
import { Button, EmptyState, Field, Sheet, Tabs } from "@aether-ui/ui";
import { useReducer, useState } from "react";
import { Action, RunNote, StandingNotice, useRun, type Run } from "../controls/kit.tsx";
import { EFFORTS } from "../controls/commands.ts";
import { invariant } from "../fleet/invariant.ts";
import { ReadNote } from "../memory/shared.tsx";
import { configuredModels, workspaceValidation } from "./data.ts";
import { UndeliveredNote } from "./UndeliveredFirst.tsx";
import { useFirstSend } from "./firstMessage.ts";
import { EDITING, firstMessageProblem, startChatStep, type ChatSettings, type StartChat } from "./startChat.ts";
import { Workspace } from "./Workspace.tsx";

type Mode = "chat" | "agent" | "task";
type Started = Exclude<StartChat, { phase: "editing" }>;
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

const chatHref = (conn: string, agent: string): string => routeHref("#/agents/:conn/:agent", { conn, agent });

function createdAgent(reply: Record<string, unknown>): string {
  const agent = reply.data as { id?: unknown } | null;
  invariant(reply.reply === "agent" && typeof agent?.id === "string", "create_agent answers with the agent it made");
  return agent.id;
}

/**
 * Drives one chat start through its state model: create, then send the first message as typed;
 * the chat opens once that message is confirmed, and a retry resends it to the same agent.
 */
function useChatStart(conn: string, close: () => void) {
  const [state, dispatch] = useReducer(startChatStep, EDITING);
  const create = useRun(conn);
  const first = useFirstSend(conn);
  const deliver = async (agent: string, text: string) => {
    const result = await first.send(agent, text);
    dispatch({ type: "sent", result });
    if (result.outcome !== "sent") return;
    close();
    navigate(chatHref(conn, agent));
  };
  const begin = async (settings: ChatSettings, text: string) => {
    dispatch({ type: "create", settings, text });
    const command = createCommand({ mode: "chat", ...settings }, text);
    const { reply, status } = await create.attempt(command, "Started");
    if (reply === null) {
      dispatch(status.state === "unconfirmed" ? { type: "create-lost", reason: status.message } : { type: "create-failed" });
      return;
    }
    const agent = createdAgent(reply);
    dispatch({ type: "created", agent });
    await deliver(agent, text);
  };
  const retry = async () => {
    invariant(state.phase === "refused" || state.phase === "unconfirmed", "only an undelivered first message is retried");
    dispatch({ type: "retry" });
    await deliver(state.agent, state.text);
  };
  return { state, begin, retry, restart: () => dispatch({ type: "restart" }), create, first };
}

/** A started chat: its fixed settings, its first message and what became of it, and what to do next. */
function StartedChat({ conn, state, retry, restart, close, busy }: {
  conn: string; state: Started; retry: () => void; restart: () => void; close: () => void; busy: boolean;
}) {
  const { settings } = state;
  invariant(settings.workspace.startsWith("/"), "a started chat has its workspace");
  const agent = "agent" in state ? state.agent : null;
  const undelivered = state.phase === "refused" || state.phase === "unconfirmed";
  return <div className="b3-stack" aria-label="Started chat">
    <dl className="b3-facts" aria-label="Chat settings, fixed once the chat exists">
      <div><dt>Name</dt><dd>{settings.name.trim() || "assistant"}</dd></div>
      <div><dt>Workspace</dt><dd>{settings.workspace}</dd></div>
      <div><dt>Model</dt><dd>{settings.modelName}</dd></div>
      <div><dt>Effort</dt><dd>{settings.effort}</dd></div>
    </dl>
    {undelivered ? <UndeliveredNote record={{ text: state.text, outcome: state.phase, reason: state.reason }} keepProblem={state.keepProblem} />
      : <blockquote className="b3-quote" data-verbatim aria-label="First message, as typed">{state.text}</blockquote>}
    {state.phase === "creating" ? <p className="b3-note" role="status" data-state="busy">Starting the chat…</p> : null}
    {state.phase === "sending" ? <p className="b3-note" role="status" data-state="busy">Sending the first message…</p> : null}
    {state.phase === "create-unconfirmed" ? <p className="b3-note" role="alert" data-state="unconfirmed">
      The reply never arrived, so it is not known whether the chat was created. It waits in the <a className="ui-link" href="#/inbox">recovery review</a>.
    </p> : null}
    <div className="b3-row">
      {undelivered && agent !== null ? <>
        <Action conn={conn} command={aether.sendMessage({ to: agent, content: state.text })} label="Send it again" variant="primary" busy={busy} onRun={retry} />
        <a className="ui-link" href={chatHref(conn, agent)} onClick={close}>Open the chat</a>
      </> : null}
      {undelivered || state.phase === "create-unconfirmed" ? <Button size="sm" onClick={restart}>New chat with different settings</Button> : null}
    </div>
  </div>;
}

function useStartData(conn: string, path: string) {
  const app = useApp();
  const live = Object.values(useSelector(selectAgents(conn))).filter(a => !ENDED.has(a.status.state));
  const models = useReread(conn, "start:models", { cmd: "list_models" }, ["models"]);
  const validation = workspaceValidation(useSelector(selectRead(conn, `start:validation:${path}`))?.value);
  const retryModels = () => app.reread(conn, "start:models", { cmd: "list_models" }, ["models"]);
  return { live, models, choices: configuredModels(models?.value), validation, retryModels };
}

interface StartForm {
  readonly mode: Mode;
  readonly name: string;
  readonly objective: string;
  readonly path: string;
  readonly model: string;
  readonly effort: aether.Effort;
  readonly owner: string;
}

const BLANK: StartForm = { mode: "chat", name: "", objective: "", path: "", model: "", effort: "medium", owner: "" };

/** The command the form would send now, and why it cannot yet; a chat checks its first message first. */
function formCommand(form: StartForm, data: ReturnType<typeof useStartData>, selected: ReturnType<typeof configuredModels>[number] | undefined) {
  const workspace = data.validation?.valid ? data.validation.path ?? "" : "";
  const objective = form.objective.trim();
  if (form.mode === "task") {
    const blocked = objective === "" ? "Describe the work to start" : data.live.some(a => a.id === form.owner) ? null : "Choose an active owner";
    return { command: aether.createTask({ objective, owner: form.owner, priority: "normal" }), blocked, settings: null };
  }
  const textProblem = form.mode === "chat" ? firstMessageProblem(form.objective) : objective === "" ? "Describe the work to start" : null;
  const blocked = textProblem ?? (!selected ? "Choose a configured model" : workspace.startsWith("/") ? null : "Validate the workspace first");
  if (blocked !== null || !selected) return { command: aether.createAgent({ role: "assistant" }), blocked, settings: null };
  const settings: ChatSettings = { name: form.name, workspace, model: selected.model, modelName: selected.name, effort: form.effort };
  return { command: createCommand({ mode: form.mode, ...settings }, objective), blocked: null, settings };
}

/** The editable start form: what to start, its first message or objective, and its settings. */
function StartFormView({ conn, form, change, run, onStart }: {
  conn: string; form: StartForm; change: (patch: Partial<StartForm>) => void; run: Run;
  onStart: (settings: ChatSettings | null, command: aether.CreateAgentCommand | aether.CreateTaskCommand) => void;
}) {
  const data = useStartData(conn, form.path);
  const selected = data.choices.find(m => `${m.model.provider}/${m.model.model}` === form.model) ?? data.choices.find(m => m.is_default) ?? data.choices[0];
  const { command, blocked, settings } = formCommand(form, data, selected);
  const disabled = usePermission(conn, command).refusal !== null;
  const label = form.mode === "task" ? "Create task" : form.mode === "chat" ? "Start chat" : "Start agent";
  return <>
    <Tabs label="Work type" selected={form.mode} onSelect={id => change({ mode: id as Mode })} tabs={[{ id: "chat", label: "Chat", panel: null }, { id: "agent", label: "Task agent", panel: null }, { id: "task", label: "Existing agent task", panel: null }]} />
    <p className="b3-section-hint">{MODE_HINTS[form.mode]}</p>
    <Field label={form.mode === "chat" ? "First message" : "Objective"} value={form.objective} disabled={disabled} onChange={e => change({ objective: e.target.value })} />
    {form.mode === "task" ? <div className="b3-row">{data.live.map(a => <Button key={a.id} disabled={disabled} aria-pressed={form.owner === a.id} onClick={() => change({ owner: a.id })}>{a.name}</Button>)}</div> : <>
      <Field label="Name" value={form.name} disabled={disabled} onChange={e => change({ name: e.target.value })} />
      <Workspace conn={conn} path={form.path} change={path => change({ path })} />
      <ReadNote entry={data.models} retry={data.retryModels} />
      <div className="b3-row">{data.choices.map(m => <Button key={`${m.model.provider}/${m.model.model}`} disabled={disabled} aria-pressed={selected === m} onClick={() => change({ model: `${m.model.provider}/${m.model.model}` })}>{m.name}</Button>)}</div>
      {data.models?.issued && data.choices.length === 0 ? <EmptyState title="Connect a model provider on your host" /> : null}
      <div className="b3-row" aria-label="Effort">{EFFORTS.map(value => <Button key={value} disabled={disabled} aria-pressed={form.effort === value} onClick={() => change({ effort: value })}>{value}</Button>)}</div>
    </>}
    <Action conn={conn} command={command} label={label} blocked={blocked} busy={run.status.state === "busy"} onRun={() => onStart(settings, command)} />
  </>;
}

export function StartAgent({ conn, close }: { conn: string; close: () => void }) {
  const run = useRun(conn);
  const [form, setForm] = useState<StartForm>(BLANK);
  const chat = useChatStart(conn, close);
  const change = (patch: Partial<StartForm>) => setForm(current => ({ ...current, ...patch }));
  const onStart = (settings: ChatSettings | null, command: aether.CreateAgentCommand | aether.CreateTaskCommand) => {
    if (form.mode !== "chat") return run.send(command);
    invariant(settings !== null, "a chat starts from validated settings");
    return chat.begin(settings, form.objective.trim());
  };
  const busy = chat.create.status.state === "busy" || chat.first.run.status.state === "busy";
  return <Sheet open onClose={close} title="Start an agent"><div className="b3-stack"><StandingNotice conn={conn} />
    {chat.state.phase === "editing"
      ? <><StartFormView conn={conn} form={form} change={change} run={run} onStart={onStart} />
        <RunNote status={form.mode === "chat" ? chat.create.status : run.status} /></>
      : <StartedChat conn={conn} state={chat.state} retry={() => void chat.retry()} restart={chat.restart} close={close} busy={busy} />}
  </div></Sheet>;
}
