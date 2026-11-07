// Imports an issue as Archon work through `import_issue` (D2): a preview of what Archon will read, then a
// confirm. Archon keys the import by the issue, so a repeat returns the first item rather than a second.

import { connectionDraftKey, routeHref, useCommand, useDraft, usePermission } from "@aether-ui/core";
import { archon } from "@aether-ui/protocol";
import { Badge, Button, Dialog, Picker } from "@aether-ui/ui";
import { useId, useState } from "react";
import { githubRef, invariant, type PageRef } from "./ref.ts";

const PRIORITIES: readonly archon.Priority[] = ["low", "normal", "high", "critical"];

export interface ImportProps {
  readonly conn: string;
  readonly page: PageRef;
  /** Whether archond serves `github_import`. */
  readonly served: boolean;
  /** The cached issue, when Archon's cache has read it. */
  readonly object: archon.GithubObject | null;
  /** The item this issue already became, when a link says so. */
  readonly imported: { readonly id: string; readonly title: string } | null;
  readonly startOpen: boolean;
  readonly onDone: () => void;
}

/** Why the import cannot be sent now, or null when it can: connection and role first, as core checks them. */
export function importRefusal(roleRefusal: string | null, served: boolean, imported: ImportProps["imported"]): string | null {
  if (roleRefusal !== null) return roleRefusal;
  if (!served) return "This archond cannot import issues: it does not serve github_import";
  if (imported !== null) return `Already Archon work: ${imported.title}`;
  return null;
}

type Outcome = { readonly ok: true; readonly id: string; readonly title: string } | { readonly ok: false; readonly text: string };

function itemOf(reply: Record<string, unknown>): { id: string; title: string } {
  const data = reply.data as { id?: unknown; title?: unknown } | null | undefined;
  if (reply.reply !== "item" || typeof data?.id !== "string") throw new Error(`Expected the imported item, got a ${String(reply.reply)} reply`);
  return { id: data.id, title: typeof data.title === "string" ? data.title : data.id };
}

function Preview({ page, object }: Pick<ImportProps, "page" | "object">) {
  const rows: [string, string][] = [
    ["Issue", `${page.repo}#${page.number}`],
    ["Title", object?.title ?? "Read from GitHub when Archon imports it"],
    ["State", object ? (object.state === "open" ? "Open" : "Closed") : "Not read yet"],
  ];
  if (object?.author) rows.push(["Author", object.author]);
  return (
    <div className="gh-preview">
      <dl className="gh-rows">{rows.map(([term, value]) => <div key={term} className="gh-row"><dt>{term}</dt><dd>{value}</dd></div>)}</dl>
      {object && object.labels.length > 0 ? <div className="gh-labels">{object.labels.map((label) => <Badge key={label}>{label}</Badge>)}</div> : null}
      {object && object.state !== "open" ? <p className="gh-warning">This issue is closed on GitHub.</p> : null}
      <p className="gh-muted">The issue's text is untrusted: Archon reads it as the brief, and it grants no authority.</p>
    </div>
  );
}

export function ImportAction(props: ImportProps) {
  const { conn, page, served, object, imported, startOpen, onDone } = props;
  invariant(object === null || object.ref.kind === "issue", "only an issue is imported");
  const ref = githubRef(page, "issue");
  const send = useCommand(conn);
  const permission = usePermission(conn, archon.importIssue({ ref }));
  const refusal = importRefusal(permission.refusal, served, imported);
  const [open, setOpen] = useState(startOpen && refusal === null);
  const { value: draft, update, clear } = useDraft(connectionDraftKey("github", conn, `import-${page.host}-${page.repo}-${page.number}`), { priority: "normal" as archon.Priority });
  const priority = draft.priority;
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const reasonId = useId();
  const confirm = async () => {
    invariant(refusal === null, "a refused import is never sent");
    setBusy(true);
    try {
      const item = itemOf(await send({ ...archon.importIssue({ ref, priority }) }));
      setOutcome({ ok: true, ...item });
      setOpen(false);
      clear();
    } catch (error) {
      setOutcome({ ok: false, text: error instanceof Error ? error.message : String(error) });
    } finally {
      setBusy(false);
      onDone();
    }
  };
  return (
    <div className="gh-import">
      <Button variant="primary" disabled={refusal !== null || busy} aria-describedby={refusal ? reasonId : undefined} onClick={() => { setOutcome(null); setOpen(true); }}>
        Import as Archon work
      </Button>
      {refusal ? <p id={reasonId} className="gh-muted gh-reason">{refusal}</p> : null}
      {outcome?.ok ? <p role="status" className="gh-muted">Imported as <a className="ui-link" href={routeHref("#/archon/items/:item", { item: outcome.id })}>{outcome.title}</a></p> : null}
      {outcome && !outcome.ok ? <p role="alert" className="gh-error">The import failed: {outcome.text}</p> : null}
      <Dialog open={open} onClose={() => setOpen(false)} title={`Import #${page.number} as Archon work?`} description="Archon queues the issue as an item on its repository.">
        <Preview page={page} object={object} />
        <Picker label="Priority" value={priority} data-testid="gh-import-priority" onChange={(priority) => update({ priority: priority as archon.Priority })}
          options={PRIORITIES.map((id) => ({ id, label: id }))} />
        <div className="gh-confirm">
          <Button onClick={() => setOpen(false)}>Not now</Button>
          <Button variant="primary" data-autofocus disabled={busy || refusal !== null} onClick={() => void confirm()}>{busy ? "Importing…" : "Import"}</Button>
        </div>
      </Dialog>
    </div>
  );
}
