// Starting work: a chat (the default) is a task-less interactive assistant that waits for the next
// message after each reply, as the TUI starts one; a task agent finishes with its objective.

import { connectionDraftKey, drafts, navigate, routeHref, selectAgents, selectRead, useApp, useDraft, usePermission, useReread, useSelector, type DraftValue } from "@aether-ui/core";
import { aether } from "@aether-ui/protocol";
import { Button, EmptyState, Field, Sheet, Tabs } from "@aether-ui/ui";
import { useReducer, useRef, useState } from "react";
import { Action, RunNote, StandingNotice, useRun, type Run } from "../controls/kit.tsx";

import { invariant } from "../fleet/invariant.ts";
import { ReadNote } from "../memory/shared.tsx";
import { configuredModels, createdAgent, EFFORTS, modelGroups, modelKey, networkDefault, pickModel, recentWorkspaces, workspaceValidation } from "./data.ts";
import { SelectField, TextAreaField } from "./fields.tsx";
import { UndeliveredNote } from "./UndeliveredFirst.tsx";
import { useFirstSend } from "./firstMessage.ts";
import { EDITING, firstMessageProblem, startChatStep, type ChatSettings, type StartChat } from "./startChat.ts";
import { Workspace } from "./Workspace.tsx";
import "./activity.css";

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
  readonly network?: aether.NetworkPolicy;
}

/** The create command for a chat or a task agent; only a task agent carries an objective. */
export function createCommand(draft: NewAgentDraft, objective: string): aether.CreateAgentCommand {
  invariant(draft.workspace.startsWith("/"), "an agent starts in a validated absolute workspace");
  invariant(objective.trim() !== "", "a new agent starts with a message or an objective");
  const common = { start: true, scope: { workspace_root: draft.workspace, ...(draft.network ? { network: draft.network } : {}) }, model: draft.model, approval: { require_for: ["external" as const] } };
  if (draft.mode === "agent") {
    return aether.createAgent({ ...common, name: draft.name.trim() || null, role: "worker", objective: objective.trim(), config: { effort: draft.effort } });
  }
  return aether.createAgent({ ...common, name: draft.name.trim() || "assistant", role: "assistant", config: { effort: draft.effort, interactive: true } });
}

const chatHref = (conn: string, agent: string): string => routeHref("#/agents/:conn/:agent", { conn, agent });

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
    const agent = createdAgent(reply).id;
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

function useStartData(conn: string, path: string | null) {
  const app = useApp();
  const live = Object.values(useSelector(selectAgents(conn))).filter(a => !a.archived && !ENDED.has(a.status.state));
  const models = useReread(conn, "start:models", { cmd: "list_models" }, ["models"]);
  const recent = useReread(conn, "start:workspaces", { cmd: "list_workspaces" }, ["workspaces"]);
  const network = networkDefault(useReread(conn, "start:status", { cmd: "status" }, ["status"])?.value);
  const effectivePath = path ?? recentWorkspaces(recent?.value)[0]?.path ?? "";
  const validation = workspaceValidation(useSelector(selectRead(conn, `start:validation:${effectivePath}`))?.value);
  const retryModels = () => app.reread(conn, "start:models", { cmd: "list_models" }, ["models"]);
  return { live, models, recent, network, effectivePath, choices: configuredModels(models?.value), validation, retryModels };
}

interface StartForm {
  readonly [field: string]: unknown;
  readonly mode: Mode;
  readonly name: string;
  readonly objective: string;
  readonly path: string | null;
  readonly model: string;
  readonly effort: aether.Effort;
  readonly owner: string;
}

const BLANK: StartForm = { mode: "chat", name: "", objective: "", path: null, model: "", effort: "medium", owner: "" };

/** The command the form would send now, and why it cannot yet; a chat checks its first message first. */
function formCommand(form: StartForm, data: ReturnType<typeof useStartData>, selected: ReturnType<typeof configuredModels>[number] | undefined) {
  const workspace = data.validation?.valid ? data.validation.path ?? "" : "";
  const objective = form.objective.trim();
  if (form.mode === "task") {
    const blocked = objective === "" ? "Describe the work to start" : form.owner === "" || data.live.some(a => a.id === form.owner && !a.current_task) ? null : "Choose an available owner";
    return { command: aether.createTask({ objective, owner: form.owner || null, priority: "normal" }), blocked, settings: null };
  }
  const textProblem = form.mode === "chat" ? firstMessageProblem(form.objective) : objective === "" ? "Describe the work to start" : null;
  const blocked = textProblem ?? (!selected ? "Choose a configured model" : !data.effectivePath ? "Choose a workspace" : !data.validation ? "Checking the workspace…" : !workspace.startsWith("/") ? data.validation.reason ?? "Choose a valid workspace" : data.network === undefined ? "Reading this computer's settings…" : null);
  if (blocked !== null || !selected || data.network === undefined) return { command: aether.createAgent({ role: "assistant" }), blocked, settings: null };
  const settings: ChatSettings = { name: form.name, workspace, model: selected.model, modelName: selected.name, effort: form.effort, network: data.network };
  return { command: createCommand({ mode: form.mode, ...settings }, objective), blocked: null, settings };
}

/** The editable start form: what to start, its first message or objective, and its settings. */
function StartFormView({ conn, form, change, run, onStart, close }: {
  conn: string; form: StartForm; change: (patch: Partial<StartForm>) => void; run: Run; close: () => void;
  onStart: (settings: ChatSettings | null, command: aether.CreateAgentCommand | aether.CreateTaskCommand) => void;
}) {
  const data = useStartData(conn, form.path);
  const selected = pickModel(data.choices, form.model);
  const { command, blocked, settings } = formCommand(form, data, selected);
  const disabled = usePermission(conn, command).refusal !== null;
  const label = form.mode === "task" ? "Create task" : form.mode === "chat" ? "Start chat" : "Start agent";
  const footer = <div className="start-footer"><RunNote status={run.status} />
    <Action conn={conn} command={command} label={label} variant="primary" data-testid="start-submit" blocked={blocked} busy={run.status.state === "busy" || (form.mode !== "chat" && run.status.state === "done")} onRun={() => onStart(settings, command)} /></div>;
  return <Sheet open onClose={close} title="Start an agent" footer={footer}><div className="start-form"><StandingNotice conn={conn} />
    <div data-testid="start-mode"><Tabs label="Work type" selected={form.mode} onSelect={id => change({ mode: id as Mode })} tabs={[{ id: "chat", label: "Chat", panel: null }, { id: "agent", label: "Task agent", panel: null }, { id: "task", label: "Existing agent task", panel: null }]} /></div>
    <p className="b3-section-hint">{MODE_HINTS[form.mode]}</p>
    <TextAreaField label={form.mode === "chat" ? "First message" : "What should the agent do?"} testId="start-objective" value={form.objective} disabled={disabled} autoFocus onChange={objective => change({ objective })} />
    {form.mode === "task" ? <SelectField label="Agent" testId="start-owner" value={form.owner} disabled={disabled} onChange={owner => change({ owner })}>
      <option value="">Unassigned</option>{data.live.map(a => <option key={a.id} value={a.id} disabled={!!a.current_task}>{a.name}{a.current_task ? " — already owns a task" : ""}</option>)}</SelectField> : <>
      <Workspace conn={conn} path={data.effectivePath} change={path => change({ path })} recent={data.recent} />
      <ReadNote entry={data.models} retry={data.retryModels} />
      {data.models?.issued && data.choices.length === 0 ? <EmptyState title="No model is ready yet" action={<a className="ui-link" href="#/settings/providers">Open providers</a>}>Sign in to a model provider to start agents.</EmptyState> :
        <SelectField label="Model" testId="start-model" value={selected ? modelKey(selected) : ""} disabled={disabled || data.choices.length === 0} onChange={model => change({ model })}>
          {modelGroups(data.choices).map(g => <optgroup key={g.provider} label={g.provider}>{g.models.map(m => <option key={modelKey(m)} value={modelKey(m)}>{m.name}</option>)}</optgroup>)}</SelectField>}
      <SelectField label="Effort" value={form.effort} disabled={disabled} onChange={effort => change({ effort: effort as aether.Effort })}>
        {EFFORTS.map(e => <option key={e.value} value={e.value}>{e.label}</option>)}</SelectField>
      <Field label="Name" hint="Optional; the agent is named for you when blank" value={form.name} disabled={disabled} onChange={e => change({ name: e.target.value })} />
    </>}
  </div></Sheet>;
}

export function StartAgent({ conn, close }: { conn: string; close: () => void }) {
  const run = useRun(conn);
  const app = useApp();
  const agents = useSelector(selectAgents(conn));
  const draftKey = connectionDraftKey("activity", conn, "start-agent");
  const draft = useDraft(draftKey, BLANK);
  const form = draft.value;
  // The stored draft a chat start submitted: every edit stores a new object, so a newer draft is left alone.
  const submitted = useRef<DraftValue | undefined>(undefined);
  const chat = useChatStart(conn, () => {
    if (drafts.get(draftKey) === submitted.current) drafts.clear(draftKey);
    close();
  });
  const change = (patch: Partial<StartForm>) => draft.update(patch);
  const onStart = async (settings: ChatSettings | null, command: aether.CreateAgentCommand | aether.CreateTaskCommand) => {
    if (form.mode !== "chat") {
      const reply = await run.send(command, "Starting…");
      if (reply === null) return;
      draft.clearIfUnchanged();
      if (form.mode === "task" && !form.owner) {
        close();
        app.store.report("Created unassigned task", conn, undefined, "success");
        return;
      }
      const agent = form.mode === "task" ? { id: form.owner, name: agents[form.owner]?.name ?? form.owner } : createdAgent(reply);
      close();
      app.store.report(form.mode === "task" ? `Started task for ${agent.name}` : `Started ${agent.name}`, conn, undefined, "success");
      navigate(chatHref(conn, agent.id));
      return;
    }
    invariant(settings !== null, "a chat starts from validated settings");
    submitted.current = drafts.get(draftKey);
    return chat.begin(settings, form.objective.trim());
  };
  const busy = chat.create.status.state === "busy" || chat.first.run.status.state === "busy";
  if (chat.state.phase === "editing") return <StartFormView conn={conn} form={form} change={change} run={form.mode === "chat" ? chat.create : run} onStart={onStart} close={close} />;
  return <Sheet open onClose={close} title="Start an agent"><div className="start-form"><StandingNotice conn={conn} />
    <StartedChat conn={conn} state={chat.state} retry={() => void chat.retry()} restart={chat.restart} close={close} busy={busy} />
  </div></Sheet>;
}

/** Opens the Start sheet from a screen of its own, such as the fleet. */
export function StartAgentButton({ conn, label = "Start an agent" }: { conn: string; label?: string }) {
  const [open, setOpen] = useState(false);
  return <>
    <Button onClick={() => setOpen(true)}>{label}</Button>
    {open && <StartAgent conn={conn} close={() => setOpen(false)} />}
  </>;
}
