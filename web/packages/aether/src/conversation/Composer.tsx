// The message composer under the transcript: the draft survives leaving the screen and is cleared
// only once the daemon accepted the message. It sends the same send_message the Steer control does.

import { selectAgent, selectConnection, useDraft, usePermission, useSelector, type State } from "@aether-ui/core";
import type { aether, ConnId, ConnectionInfo } from "@aether-ui/protocol";
import { Button, EntityActionMenu, Switch, TextArea } from "@aether-ui/ui";
import { useCallback, useMemo, type FormEvent, type KeyboardEvent } from "react";
import { sendCommand } from "../controls/commands.ts";
import { useRun, type RunStatus } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";

type ComposerDraft = { readonly text: string; readonly subtree: boolean };
const INITIAL: ComposerDraft = Object.freeze({ text: "", subtree: false });
const FINISHED: Readonly<Partial<Record<aether.AgentStatusKind, string>>> = { completed: "has completed", failed: "has failed", cancelled: "was cancelled" };

export const composerKey = (conn: ConnId, agent: string): string => `agent/${conn}/${agent}/composer`;

/** Why nothing can be sent right now, or null when a message can go. */
function blockedReason(agent: aether.Agent | undefined, info: ConnectionInfo | undefined, refusal: string | null): string | null {
  if (info === undefined || info.state !== "ready") return `Waiting for ${info?.label ?? "the connection"}; you can send once it is connected again.`;
  if (agent === undefined) return "This agent is not in the fleet, so it cannot be sent a message.";
  const finished = FINISHED[agent.status.state];
  if (finished !== undefined) return `${agent.name} ${finished} and reads no more messages.`;
  return refusal;
}

/** Whether any agent was delegated by this one, which is when the subtree switch means something. */
function useHasDelegates(conn: ConnId, agent: string): boolean {
  return useSelector(useCallback((state: State) => {
    const agents = state.aether[conn]?.agents;
    return agents !== undefined && Object.values(agents).some((other) => other.parent === agent);
  }, [conn, agent]));
}

function useComposer(conn: ConnId, agentId: string, onSent: (() => void) | undefined) {
  const agent = useSelector(useMemo(() => selectAgent(conn, agentId), [conn, agentId]));
  const info = useSelector(useMemo(() => selectConnection(conn), [conn]));
  const { refusal } = usePermission(conn, { cmd: "send_message", to: agentId });
  const delegates = useHasDelegates(conn, agentId);
  const draft = useDraft<ComposerDraft>(composerKey(conn, agentId), INITIAL);
  const run = useRun(conn);
  const blocked = blockedReason(agent, info, refusal);
  const busy = run.status.state === "busy";
  const { text, subtree } = draft.value;
  const empty = text.trim() === "";
  const send = useCallback(async () => {
    if (busy || empty || blocked !== null) return;
    const reply = await run.send(sendCommand(agentId, text, subtree && delegates), "Sent");
    if (reply === null) return;
    invariant(typeof reply === "object", "an accepted message has a reply");
    draft.clearIfUnchanged();
    onSent?.();
  }, [busy, empty, blocked, run, agentId, text, subtree, delegates, draft, onSent]);
  return { agent, draft, run, blocked, busy, empty, delegates, send };
}

/** What became of the last send: an inline error with a retry, never a toast of its own. */
function SendNote({ status, empty, retry }: { readonly status: RunStatus; readonly empty: boolean; readonly retry: () => void }) {
  if (status.state === "failed") {
    return (
      <p className="conv-composer-note" role="alert" data-state="failed">
        Not sent: {status.message} <Button size="sm" variant="quiet" data-testid="composer-retry" onClick={retry}>Retry</Button>
      </p>
    );
  }
  if (status.state === "unconfirmed") {
    return <p className="conv-composer-note" role="alert" data-state="unconfirmed">{status.message} <a className="ui-link" href="#/inbox">Open the recovery review</a></p>;
  }
  if (status.state === "done" && empty) return <p className="conv-composer-note" role="status" data-state="done">Sent. It reaches the agent at its next turn.</p>;
  return null;
}

/** `onSent` runs once the daemon accepted a message, so the screen can follow it to the end of the transcript. */
export function Composer({ conn, agent: agentId, onSent }: { readonly conn: ConnId; readonly agent: string; readonly onSent?: () => void }) {
  invariant(conn !== "" && agentId !== "", "a composer names its connection and agent");
  const { agent, draft, run, blocked, busy, empty, delegates, send } = useComposer(conn, agentId, onSent);
  const finished = agent !== undefined && FINISHED[agent.status.state] !== undefined;
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Enter" || event.shiftKey || event.altKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    void send();
  };
  const onSubmit = (event: FormEvent) => {
    event.preventDefault();
    void send();
  };
  const name = agent?.name ?? agentId;
  return (
    <form className="conv-composer" data-testid="conv-composer" aria-label={`Message ${name}`} onSubmit={onSubmit} data-blocked={blocked === null ? undefined : "true"}>
      <TextArea label={`Message to ${name}`} hideLabel rows={2} value={draft.value.text} data-testid="composer-input"
        placeholder={blocked === null ? `Message ${name}… (Enter sends, Shift+Enter adds a line)` : "Sending is off"}
        disabled={blocked !== null} readOnly={busy} aria-busy={busy || undefined}
        onChange={(event) => draft.update({ text: event.target.value })} onKeyDown={onKeyDown} />
      <div className="conv-composer-bar">
        {delegates ? <Switch className="conv-composer-switch" label="Also wake delegated agents" checked={draft.value.subtree} disabled={blocked !== null} onChange={(subtree) => draft.update({ subtree })} /> : null}
        <span className="conv-composer-keys" aria-hidden="true"><kbd className="ui-kbd">Enter</kbd> send · <kbd className="ui-kbd">Shift</kbd>+<kbd className="ui-kbd">Enter</kbd> new line</span>
        <Button variant="primary" type="submit" data-testid="composer-send" disabled={empty || busy || blocked !== null}>{busy ? "Sending…" : "Send"}</Button>
      </div>
      {blocked === null ? null : (
        <p className="conv-composer-note" role="note" data-state="blocked">
          {blocked}{finished ? <span className="conv-composer-actions"><EntityActionMenu entity={{ kind: "agent", conn, id: agentId }} label="Agent actions" /></span> : null}
        </p>
      )}
      <SendNote status={run.status} empty={empty} retry={() => void send()} />
    </form>
  );
}
