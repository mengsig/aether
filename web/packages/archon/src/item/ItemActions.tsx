// What the operator can do to an item from its page: cancel, retry, and pause or resume its repository or
// pipeline. Cancel and retry ask first, as the retired app did; a refusal leaves every control disabled.

import type { archon } from "@aether-ui/protocol";
import { Button, Dialog } from "@aether-ui/ui";
import { useState } from "react";
import { useChange, type ArchonAccess } from "../queue/archon.ts";
import { invariant, type StateKind } from "../queue/model.ts";
import { canCancel, canRetry } from "./facts.ts";
import { SteerItem } from "./SteerItem.tsx";
import { ItemPriority } from "./ItemPriority.tsx";

interface Confirm { readonly title: string; readonly message: string; readonly label: string; readonly command: Record<string, unknown> }

export interface ItemActionsProps {
  readonly access: ArchonAccess;
  readonly item: archon.QueueItem;
  readonly kind: StateKind;
  readonly repoName: string | null;
  readonly pause: archon.PauseState | null;
  /** Rereads what the change touched; on protocol 24 the change stream does it too. */
  readonly onDone: () => void;
}

export function ItemActions({ access, item, kind, repoName, pause, onDone }: ItemActionsProps) {
  const change = useChange(access);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const run = async (command: Record<string, unknown>, done: string) => {
    invariant(command.cmd !== undefined, "an action sends a command");
    setBusy(true);
    const result = await change(command);
    setBusy(false);
    setConfirm(null);
    setMessage(result.ok ? { ok: true, text: done } : { ok: false, text: result.error });
    onDone();
  };
  const disabled = busy || access.refusal !== null;
  const title = access.refusal ?? undefined;
  const repoPaused = item.repo ? pause?.repos.includes(item.repo) ?? false : false;
  const pipelinePaused = pause?.pipelines.includes(item.pipeline) ?? false;
  const pauseCommand = (scope: "repo" | "pipeline", name: string, paused: boolean) => ({ cmd: paused ? "resume" : "pause", scope, name });
  return (
    <div className="item-actions" role="group" aria-label="Item actions">
      <SteerItem access={access} item={item} active={canCancel(kind)} disabled={disabled || !access.live} onDone={onDone} />
      {kind === "queued" ? <ItemPriority access={access} item={item} disabled={disabled || !access.live} onDone={onDone} /> : null}
      {canCancel(kind) && !access.live ? <p className="item-muted">Steering and editing require protocol 24.</p> : null}
      {canCancel(kind) && access.refusal ? <p className="item-muted">{access.refusal}</p> : null}
      {canCancel(kind) ? <Button variant="danger" disabled={disabled} title={title} onClick={() => setConfirm({
        title: "Cancel this item?", message: `Archon stops “${item.title}” and its agents. You can retry it later.`, label: "Cancel item",
        command: { cmd: "cancel_item", item: item.id, reason: null },
      })}>Cancel</Button> : null}
      {canRetry(kind) ? <Button variant="primary" disabled={disabled} title={title} onClick={() => setConfirm({
        title: "Retry this item?", message: `Archon queues “${item.title}” again from a fresh run.`, label: "Retry",
        command: { cmd: "retry_item", item: item.id, model_plan: null },
      })}>Retry</Button> : null}
      {item.repo && pause ? (
        <Button disabled={disabled} title={title} aria-label={`${repoPaused ? "Resume" : "Pause"} repository ${repoName ?? item.repo}`}
          onClick={() => void run(pauseCommand("repo", item.repo as string, repoPaused), `${repoPaused ? "Resumed" : "Paused"} ${repoName ?? item.repo}`)}>
          {repoPaused ? "Resume repository" : "Pause repository"}
        </Button>
      ) : null}
      {pause ? (
        <Button disabled={disabled} title={title} aria-label={`${pipelinePaused ? "Resume" : "Pause"} pipeline ${item.pipeline}`}
          onClick={() => void run(pauseCommand("pipeline", item.pipeline, pipelinePaused), `${pipelinePaused ? "Resumed" : "Paused"} ${item.pipeline}`)}>
          {pipelinePaused ? "Resume pipeline" : "Pause pipeline"}
        </Button>
      ) : null}
      {message ? <p role={message.ok ? "status" : "alert"} className={message.ok ? "item-muted" : "item-error"}>{message.text}</p> : null}
      <Dialog open={confirm !== null} onClose={() => setConfirm(null)} title={confirm?.title ?? "Confirm"} {...(confirm ? { description: confirm.message } : {})}>
        <div className="item-confirm">
          <Button onClick={() => setConfirm(null)}>Not now</Button>
          <Button variant={confirm?.command.cmd === "cancel_item" ? "danger" : "primary"} disabled={disabled} data-autofocus
            onClick={() => { if (confirm) void run(confirm.command, confirm.command.cmd === "cancel_item" ? "Item cancelled" : "Back in the queue"); }}>
            {confirm?.label ?? "Confirm"}
          </Button>
        </div>
      </Dialog>
    </div>
  );
}
