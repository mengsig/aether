// Queueing work and messaging the orchestrator. An enqueue carries one idempotency key per open dialog,
// so a resend after a lost reply cannot queue the item twice.

import { navigate, routeHref, useDraft } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { Button, Dialog, Field, Picker, TextArea } from "@aether-ui/ui";
import { useState, type FormEvent } from "react";
import { useChange, useRead, type ArchonAccess } from "./archon.ts";
import { invariant, PRIORITIES, priorityLabel, replyOf } from "./model.ts";

export type ItemDraft = { title: string; brief: string; repo: string; pipeline: string; priority: archon.Priority; review: string };
export const EMPTY_ITEM: ItemDraft = { title: "", brief: "", repo: "", pipeline: "", priority: "normal", review: "" };

export function enqueueCommand(draft: ItemDraft, key: string): Record<string, unknown> {
  invariant(draft.title.trim() !== "" && draft.brief.trim() !== "", "an item needs a title and brief");
  invariant(draft.repo.length > 0 && key.length > 0, "an enqueue names a repository and idempotency key");
  return {
    cmd: "enqueue", repo: draft.repo || null, pipeline: draft.pipeline || null, title: draft.title.trim(), brief: draft.brief.trim(),
    priority: draft.priority, budget_usd: null, idempotency_key: key, ...(draft.review ? { autonomy: { review: draft.review } } : {}),
  };
}

function ItemChoices({ draft, set, repos, catalog }: {
  readonly draft: ItemDraft; readonly set: (patch: Partial<ItemDraft>) => void;
  readonly repos: readonly archon.Repo[]; readonly catalog: archon.CatalogListing | undefined;
}) {
  return <>
    <Picker label="Repository" data-testid="new-item-repo" value={draft.repo} placeholder="Choose a repository" error={!draft.repo ? "Choose a repository." : undefined}
      options={repos.map((repo) => ({ id: repo.id, label: repo.name, description: repo.path }))}
      onChange={(repo) => set({ repo, pipeline: repos.find((entry) => entry.id === repo)?.default_pipeline ?? "" })} />
    <Picker label="Pipeline" data-testid="new-item-pipeline" value={draft.pipeline} onChange={(pipeline) => set({ pipeline })}
      options={[{ id: "", label: "The repository's default" }, ...(catalog?.pipelines ?? []).map((pipeline) => ({ id: pipeline.name, label: pipeline.name }))]} />
    <Picker label="Priority" data-testid="new-item-priority" value={draft.priority} onChange={(priority) => set({ priority: priority as archon.Priority })}
      options={PRIORITIES.map((id) => ({ id, label: priorityLabel(id) }))} />
    <Picker label="Review" data-testid="new-item-review" value={draft.review} onChange={(review) => set({ review })}
      options={[{ id: "", label: "Default" }, ...["strict", "standard", "none"].map((id) => ({ id, label: id[0]?.toUpperCase() + id.slice(1) }))]} />
  </>;
}

export function NewItemDialog({ access, open, repos, onClose }: {
  readonly access: ArchonAccess; readonly open: boolean; readonly repos: readonly archon.Repo[]; readonly onClose: () => void;
}) {
  const change = useChange(access);
  const { value: draft, update: set, clear } = useDraft("archon/new-item", EMPTY_ITEM);
  const { value: attempt, update: saveAttempt, clear: clearAttempt } = useDraft("archon/new-item-attempt", { key: "" });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const catalog = useRead<archon.CatalogListing>(access, open ? { cmd: "list_pipelines", repo: draft.repo || null } : null, ["catalog"]);
  const valid = !!draft.title.trim() && !!draft.brief.trim() && repos.some((repo) => repo.id === draft.repo);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!valid || busy || access.refusal) return;
    setBusy(true);
    const key = attempt.key || crypto.randomUUID();
    saveAttempt({ key });
    const result = await change(enqueueCommand(draft, key));
    setBusy(false);
    if (!result.ok) return setError(result.error);
    const item = replyOf<archon.QueueItem>(result.reply, "item", "item_budgets_off");
    clear(); clearAttempt(); setError(null); onClose();
    if (item && typeof item.id === "string") navigate(routeHref("#/archon/items/:item", { item: item.id }));
  };
  return (
    <Dialog open={open} onClose={onClose} title="New item" description="Archon queues it and starts when a slot is free. Your draft is kept when you close this dialog.">
      <form className="queue-form" onSubmit={(event) => void submit(event)} noValidate>
        <Field label="Title" value={draft.title} onChange={(event) => set({ title: event.target.value })} data-autofocus data-testid="new-item-title" {...(!draft.title.trim() ? { error: "Give this item a title." } : {})} />
        <TextArea label="Brief" rows={5} value={draft.brief} onChange={(event) => set({ brief: event.target.value })} data-testid="new-item-brief" error={!draft.brief.trim() ? "Describe the work to do." : undefined} />
        <ItemChoices draft={draft} set={set} repos={repos} catalog={catalog.data} />
        {catalog.error ? <p role="alert" className="queue-error">{catalog.error}</p> : null}
        {error ? <p role="alert" className="queue-error">{error}</p> : null}
        <div className="queue-form-actions"><Button onClick={onClose}>Cancel</Button><Button type="submit" variant="primary" data-testid="new-item-submit" disabled={!valid || busy || access.refusal !== null}>Queue item</Button></div>
      </form>
    </Dialog>
  );
}

export function SayDialog({ access, open, onClose }: { readonly access: ArchonAccess; readonly open: boolean; readonly onClose: () => void }) {
  const change = useChange(access);
  const { value: message, update, clear } = useDraft("archon/message", { text: "" });
  const text = message.text;
  const setText = (text: string) => update({ text });
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const close = () => { setError(null); setSent(false); onClose(); };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!text.trim()) return setError("Write a message first.");
    const result = await change({ cmd: "say", text: text.trim() });
    if (!result.ok) return setError(result.error);
    clear();
    setError(null);
    setSent(true);
  };
  return (
    <Dialog open={open} onClose={close} title="Message Archon" description="The orchestrator reads it on its next turn.">
      <form className="queue-form" onSubmit={(event) => void submit(event)} noValidate>
        <label className="queue-select"><span className="ui-field-label">Message</span>
          <textarea className="ui-field-input" rows={4} value={text} data-autofocus onChange={(event) => { setText(event.target.value); setSent(false); }} /></label>
        {error ? <p role="alert" className="queue-error">{error}</p> : null}
        {sent ? <p role="status" className="queue-muted">Sent to the orchestrator.</p> : null}
        <div className="queue-form-actions"><Button onClick={close}>Close</Button><Button type="submit" variant="primary">Send</Button></div>
      </form>
    </Dialog>
  );
}
