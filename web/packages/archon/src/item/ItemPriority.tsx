import type { archon } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useState, type FormEvent } from "react";
import { useChange, type ArchonAccess } from "../queue/archon.ts";
import { invariant, PRIORITIES, priorityLabel } from "../queue/model.ts";

export function ItemPriority({ access, item, disabled, onDone }: { readonly access: ArchonAccess; readonly item: archon.QueueItem; readonly disabled: boolean; readonly onDone: () => void }) {
  const [priority, setPriority] = useState(item.priority);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const change = useChange(access);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    invariant(item.id.length > 0, "an update names an item");
    invariant(PRIORITIES.includes(priority), "an update uses a known priority");
    setBusy(true);
    const result = await change({ cmd: "update_item", item: item.id, patch: { priority } });
    setBusy(false);
    if (!result.ok) return setError(result.error);
    setError(null);
    onDone();
  };
  return <form className="queue-form" onSubmit={(event) => void submit(event)}>
    <label className="queue-select"><span className="ui-field-label">Priority</span>
      <select className="ui-field-input" value={priority} disabled={disabled || busy} onChange={(event) => setPriority(event.target.value as archon.Priority)}>
        {PRIORITIES.map((value) => <option key={value} value={value}>{priorityLabel(value)}</option>)}
      </select>
    </label>
    <Button type="submit" disabled={disabled || busy || priority === item.priority}>Update priority</Button>
    {error ? <p role="alert" className="queue-error">{error}</p> : null}
  </form>;
}
