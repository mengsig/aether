import { useApp, useDraft } from "@aether-ui/core";
import { archon } from "@aether-ui/protocol";
import { Button, Combo, Field, Picker, Segmented, Sheet, Switch } from "@aether-ui/ui";
import { useState, type FormEvent } from "react";
import { useChange, useRead, type ArchonAccess } from "./archon.ts";
import { invariant, replyOf } from "./model.ts";

export const EMPTY_REPOSITORY = { path: "", name: "", pipeline: "", branch: "main", ship: "pr" as archon.ShipMode, watch: false };
const SHIP_OPTIONS = [{ id: "pr", label: "Open a pull request" }, { id: "local", label: "Keep it local" }];
/** The desktop bridge keeps `github_watch` for the archon CLI, so the app only shows the switch. */
export const WATCH_DESKTOP_HINT = "Use the archon CLI to watch GitHub.";
export type RepositoryDraft = typeof EMPTY_REPOSITORY;

export function folderName(path: string): string {
  return path.trim().replace(/[\\/]+$/, "").split(/[\\/]/).at(-1) ?? "";
}

export function repositoryCommand(draft: RepositoryDraft): archon.RepoAddCommand {
  invariant(draft.path.trim().length > 0, "a repository has a path");
  invariant(draft.branch.trim().length > 0, "a repository has a base branch");
  invariant(draft.ship === "pr" || draft.ship === "local", "a repository ships by pull request or locally");
  return archon.repoAdd({ path: draft.path.trim(), name: draft.name.trim() || null, default_pipeline: draft.pipeline || null,
    base_branch: draft.branch.trim(), ship: draft.ship, github_watch: draft.watch });
}

function RepositoryFields({ draft, update, access }: {
  readonly draft: RepositoryDraft; readonly update: (patch: Partial<RepositoryDraft>) => void; readonly access: ArchonAccess;
}) {
  const app = useApp();
  const catalog = useRead<archon.CatalogListing>(access, { cmd: "list_pipelines", repo: null }, ["catalog"]);
  const [error, setError] = useState<string | null>(null);
  const pathChanged = (path: string) => update({ path, ...(draft.name === "" || draft.name === folderName(draft.path) ? { name: folderName(path) } : {}) });
  const browse = async () => {
    invariant(app.host.desktop !== undefined, "browsing requires the desktop host");
    try {
      const path = await app.host.desktop.pickDirectory("repository");
      if (path !== null) pathChanged(path);
      setError(null);
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  };
  return <>
    <Combo label="Repository path" value={draft.path} onChange={pathChanged} suggestions={[]} data-testid="repo-path"
      hint="Path to an existing checkout on the Archon host." error={!draft.path.trim() ? "Choose a repository folder." : undefined}
      trailing={app.host.desktop ? <Button data-testid="repo-path-browse" onClick={() => void browse()}>Browse</Button> : undefined} />
    <Field label="Name" value={draft.name} onChange={(event) => update({ name: event.target.value })} data-testid="repo-name" />
    <Picker label="Default pipeline" value={draft.pipeline} onChange={(pipeline) => update({ pipeline })} data-testid="repo-default-pipeline"
      options={[{ id: "", label: "Archon's default" }, ...(catalog.data?.pipelines ?? []).map((pipeline) => ({ id: pipeline.name, label: pipeline.name }))]} />
    <Field label="Base branch" value={draft.branch} onChange={(event) => update({ branch: event.target.value })} {...(!draft.branch.trim() ? { error: "Enter a base branch." } : {})} />
    <Segmented label="When work is done" value={draft.ship} onChange={(ship) => update({ ship: ship as archon.ShipMode })} options={SHIP_OPTIONS} />
    <Switch label="Watch GitHub" checked={draft.watch} onChange={(watch) => update({ watch })} disabled={app.host.desktop !== undefined}
      hint={app.host.desktop ? WATCH_DESKTOP_HINT : "Import issues and pull requests from GitHub as they change."} />
    {error || catalog.error ? <p role="alert" className="queue-error">{error ?? catalog.error}</p> : null}
  </>;
}

export function RegisterRepository({ access, open, onClose, onRegistered }: {
  readonly access: ArchonAccess; readonly open: boolean; readonly onClose: () => void; readonly onRegistered: (repo: archon.Repo) => void;
}) {
  const { value: draft, update, clear } = useDraft("archon/register-repository", EMPTY_REPOSITORY);
  const change = useChange(access);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const valid = !!draft.path.trim() && !!draft.branch.trim();
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!valid || busy || access.refusal !== null) return;
    setBusy(true);
    const result = await change({ ...repositoryCommand(draft) });
    setBusy(false);
    if (!result.ok) return setError(result.error);
    const repo = replyOf<archon.Repo>(result.reply, "repo");
    invariant(typeof repo.id === "string" && repo.id.length > 0, "registration returns a repository id");
    invariant(typeof repo.name === "string", "registration returns a repository name");
    clear(); setError(null); onRegistered(repo);
  };
  return <Sheet open={open} onClose={onClose} title="Register a repository" description="Choose a checkout, set its defaults, then queue your first item. A checkout without an origin remote can only keep its work local.">
    <form className="queue-form" onSubmit={(event) => void submit(event)} noValidate>
      {open ? <RepositoryFields draft={draft} update={update} access={access} /> : null}
      {error ? <p role="alert" className="queue-error">{error}</p> : null}
      <div className="queue-form-actions"><Button onClick={onClose}>Cancel</Button>
        <Button type="submit" variant="primary" data-testid="repo-register-submit" disabled={!valid || busy || access.refusal !== null}>Register and create item</Button></div>
    </form>
  </Sheet>;
}
