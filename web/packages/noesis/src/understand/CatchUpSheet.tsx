// "Catch me up on a repository": choose a registered repository and a range from lists, then the evidence
// sheet opens over what changed. Nothing is typed, so no id or date can be misspelled.

import { navigate, selectConnections, useApp, useCapabilities, useReread, useSelector } from "@aether-ui/core";
import type { archon, ConnectionInfo } from "@aether-ui/protocol";
import { Button, EmptyState, ErrorState, Sheet, Skeleton } from "@aether-ui/ui";
import { useEffect, useId, useState, type FormEvent } from "react";
import { errorText } from "../learn/data.ts";
import { invariant } from "../learn/model.ts";
import { understandHref } from "./bundle.ts";
import { RANGE_PRESETS, sinceFor, type RangePresetId } from "./catchup-range.ts";
import { REPOS_KEY } from "./catchup-source.ts";
import { engineeringContextOn } from "./routes.ts";
import { VISITS_KEY, visitKey } from "./resume.ts";
import { readStringMap } from "./string-map.ts";
import "./understand.css";

/** The one archond connection, as the Archon screens choose it: the ready one, else any. */
function useArchonConnection(): ConnectionInfo | undefined {
  const connections = useSelector(selectConnections);
  const archond = Object.values(connections).filter((info) => info.daemon === "archon");
  return archond.find((info) => info.state === "ready") ?? archond[0];
}

type LastVisit = { readonly state: "loading" } | { readonly state: "ready"; readonly at: string | null } | { readonly state: "error"; readonly message: string };

/** When this repository was last caught up on, from the host's key-value store. */
function useLastVisit(conn: string, repo: string): LastVisit {
  const app = useApp();
  const key = visitKey(conn, repo);
  const [read, setRead] = useState<{ key: string; visit: LastVisit } | null>(null);
  useEffect(() => {
    let live = true;
    readStringMap(app.host.storage, VISITS_KEY).then(
      (map) => { if (live) setRead({ key, visit: { state: "ready", at: map[key] ?? null } }); },
      (error: unknown) => { if (live) setRead({ key, visit: { state: "error", message: errorText(error) } }); },
    );
    return () => { live = false; };
  }, [app, key]);
  return read !== null && read.key === key ? read.visit : { state: "loading" };
}

function RangeForm({ conn, repos, onChoose }: { readonly conn: string; readonly repos: readonly archon.Repo[]; readonly onChoose: () => void }) {
  invariant(repos.length > 0, "a form is offered only when a repository is registered");
  const [chosen, setChosen] = useState(repos[0]?.id);
  const repoId = repos.find((repo) => repo.id === chosen)?.id ?? repos[0]?.id ?? "";
  const [preset, setPreset] = useState<RangePresetId>("visit");
  const [now] = useState(() => new Date());
  const visit = useLastVisit(conn, repoId);
  const ids = { repo: useId(), range: useId() };
  invariant(repoId.length > 0, "the chosen repository is one of the registered ones");
  const since = sinceFor(preset, now, visit.state === "ready" ? visit.at : null);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (visit.state === "loading") return;
    onChoose();
    navigate(understandHref({ kind: "repository", conn, id: repoId, since }));
  };
  return (
    <form className="catchup-form" onSubmit={submit} aria-label="Choose what to catch up on">
      <div className="catchup-field">
        <label className="ui-field-label" htmlFor={ids.repo}>Repository</label>
        <select id={ids.repo} className="ui-field-input" value={repoId} onChange={(event) => setChosen(event.target.value)}>
          {repos.map((repo) => <option key={repo.id} value={repo.id}>{repo.name}</option>)}
        </select>
      </div>
      <div className="catchup-field">
        <label className="ui-field-label" htmlFor={ids.range}>Range</label>
        <select id={ids.range} className="ui-field-input" value={preset} onChange={(event) => setPreset(event.target.value as RangePresetId)}>
          {RANGE_PRESETS.map((entry) => <option key={entry.id} value={entry.id}>{entry.label}</option>)}
        </select>
        <p className="learn-muted" data-testid="catchup-since">From the start of {since} (UTC){preset === "visit" && visit.state === "ready" && visit.at === null ? " — no last visit recorded, so the last 7 days" : ""}</p>
      </div>
      {visit.state === "error" && <p className="learn-callout" role="alert">Could not read your last visit, so “Since my last visit” uses the last 7 days: {visit.message}</p>}
      <div className="learn-actions">
        <Button variant="primary" type="submit" disabled={visit.state === "loading"}>Catch me up</Button>
      </div>
    </form>
  );
}

function RepositoryChoice({ info, onChoose }: { readonly info: ConnectionInfo; readonly onChoose: () => void }) {
  const [nonce, setNonce] = useState(0);
  const entry = useReread(info.id, REPOS_KEY, info.state === "ready" ? { cmd: "repo_list" } : null, ["repos"], nonce);
  const repos: unknown = entry?.value;
  if (info.state !== "ready") return <EmptyState title="Archon is not connected">Repositories are listed once the app reconnects to Archon.</EmptyState>;
  if (!Array.isArray(repos)) {
    if (entry?.error) return <ErrorState title="Could not list repositories" message={entry.error} onRetry={() => setNonce((n) => n + 1)} />;
    return <div className="ui-loading" aria-busy="true" aria-label="Loading repositories"><Skeleton height="var(--space-8)" /><Skeleton height="var(--space-8)" /></div>;
  }
  if (repos.length === 0) return <EmptyState title="No repositories yet">Register a repository with Archon, then catch up on it here.</EmptyState>;
  return <RangeForm conn={info.id} repos={repos as archon.Repo[]} onChoose={onChoose} />;
}

function CatchUpSheet({ onClose }: { readonly onClose: () => void }) {
  const info = useArchonConnection();
  return (
    <Sheet open onClose={onClose} title="Catch me up" className="catchup-sheet"
      description="Noesis teaches what changed in a repository: merged pull requests, items and its memory. You see the evidence before anything is sent.">
      {info === undefined
        ? <EmptyState title="No Archon connection">Connect or pair an archond to catch up on its repositories.</EmptyState>
        : <RepositoryChoice info={info} onChoose={onClose} />}
    </Sheet>
  );
}

/** The Learn home's entry, offered only while engineering context is on, as "Learn from this" is. */
export function CatchUpEntry() {
  const [open, setOpen] = useState(false);
  const caps = useCapabilities();
  if (!engineeringContextOn(caps)) return null;
  return (
    <section className="learn-new catchup-entry" aria-label="Catch me up on a repository">
      <h2 className="learn-subtitle">Catch me up on a repository</h2>
      <p className="learn-muted">Learn what changed since you last looked, from merged pull requests, items and what Aether remembers.</p>
      <div className="learn-actions"><Button onClick={() => setOpen(true)}>Catch me up</Button></div>
      {open && <CatchUpSheet onClose={() => setOpen(false)} />}
    </section>
  );
}
