// Queueing work and messaging the orchestrator. An enqueue carries one idempotency key per open dialog,
// so a resend after a lost reply cannot queue the item twice.

import { navigate, routeHref } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { Button, Dialog, Field } from "@aether-ui/ui";
import { useState, type FormEvent } from "react";
import { useChange, useRead, type ArchonAccess } from "./archon.ts";
import { invariant, PRIORITIES, priorityLabel, replyOf } from "./model.ts";

interface Draft { title: string; brief: string; repo: string; pipeline: string; priority: archon.Priority; review: string }
const EMPTY: Draft = { title: "", brief: "", repo: "", pipeline: "", priority: "normal", review: "" };

/** The enqueue request as archond reads it; an empty review leaves the repository's default. */
export function enqueueCommand(draft: Draft, key: string): Record<string, unknown> {
  invariant(draft.title.trim() !== "" && draft.brief.trim() !== "", "an item needs a title and brief");
  invariant(key.length > 0, "an enqueue carries an idempotency key");
  return {
    cmd: "enqueue", repo: draft.repo || null, pipeline: draft.pipeline || null, title: draft.title.trim(), brief: draft.brief.trim(),
    priority: draft.priority, budget_usd: null, idempotency_key: key, ...(draft.review ? { autonomy: { review: draft.review } } : {}),
  };
}

export function NewItemDialog({ access, open, repos, onClose }: {
  readonly access: ArchonAccess; readonly open: boolean; readonly repos: ReadonlyMap<string, string>; readonly onClose: () => void;
}) {
  const change = useChange(access);
  const [draft, setDraft] = useState<Draft>(EMPTY);
  const [key, setKey] = useState(() => crypto.randomUUID());
  const [error, setError] = useState<string | null>(null);
  const catalog = useRead<archon.CatalogListing>(access, open ? { cmd: "list_pipelines", repo: draft.repo || null } : null, ["catalog"]);
  const set = (patch: Partial<Draft>) => setDraft((current) => ({ ...current, ...patch }));
  const close = () => { setDraft(EMPTY); setError(null); setKey(crypto.randomUUID()); onClose(); };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!draft.title.trim() || !draft.brief.trim()) return setError("An item needs a title and a brief.");
    const result = await change(enqueueCommand(draft, key));
    if (!result.ok) return setError(result.error);
    const item = replyOf<archon.QueueItem>(result.reply, "item", "item_budgets_off");
    close();
    if (item && typeof item.id === "string") navigate(routeHref("#/archon/items/:item", { item: item.id }));
  };
  return (
    <Dialog open={open} onClose={close} title="New item" description="Archon queues it and starts when a slot is free.">
      <form className="queue-form" onSubmit={(event) => void submit(event)} noValidate>
        <Field label="Title" value={draft.title} onChange={(event) => set({ title: event.target.value })} data-autofocus />
        <label className="queue-select"><span className="ui-field-label">Brief</span>
          <textarea className="ui-field-input" rows={5} value={draft.brief} onChange={(event) => set({ brief: event.target.value })} /></label>
        <label className="queue-select"><span className="ui-field-label">Repository</span>
          <select className="ui-field-input" value={draft.repo} onChange={(event) => set({ repo: event.target.value, pipeline: "" })}>
            <option value="">No repository</option>
            {[...repos.entries()].map(([id, name]) => <option key={id} value={id}>{name}</option>)}
          </select></label>
        <label className="queue-select"><span className="ui-field-label">Pipeline</span>
          <select className="ui-field-input" value={draft.pipeline} onChange={(event) => set({ pipeline: event.target.value })}>
            <option value="">The repository's default</option>
            {(catalog.data?.pipelines ?? []).map((pipeline) => <option key={pipeline.name} value={pipeline.name}>{pipeline.name}</option>)}
          </select></label>
        <label className="queue-select"><span className="ui-field-label">Priority</span>
          <select className="ui-field-input" value={draft.priority} onChange={(event) => set({ priority: event.target.value as archon.Priority })}>
            {PRIORITIES.map((priority) => <option key={priority} value={priority}>{priorityLabel(priority)}</option>)}
          </select></label>
        <label className="queue-select"><span className="ui-field-label">Review</span>
          <select className="ui-field-input" value={draft.review} onChange={(event) => set({ review: event.target.value })}>
            <option value="">Default</option><option value="strict">Strict</option><option value="standard">Standard</option><option value="none">None</option>
          </select></label>
        {error ? <p role="alert" className="queue-error">{error}</p> : null}
        <div className="queue-form-actions"><Button onClick={close}>Cancel</Button><Button type="submit" variant="primary">Queue item</Button></div>
      </form>
    </Dialog>
  );
}

export function SayDialog({ access, open, onClose }: { readonly access: ArchonAccess; readonly open: boolean; readonly onClose: () => void }) {
  const change = useChange(access);
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const close = () => { setText(""); setError(null); setSent(false); onClose(); };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!text.trim()) return setError("Write a message first.");
    const result = await change({ cmd: "say", text: text.trim() });
    if (!result.ok) return setError(result.error);
    setText("");
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
