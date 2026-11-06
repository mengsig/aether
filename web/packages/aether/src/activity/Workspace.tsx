import { useEffect, useRef, useState } from "react";
import { useApp, usePermission, useReread, useSelector, selectRead } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, Combo } from "@aether-ui/ui";
import { defaultWorkspace, workspaceValidation } from "./data.ts";
import { records, ReadNote } from "../memory/shared.tsx";

interface WorkspaceProps {
  conn: string; path: string; change: (path: string) => void;
  testId?: string; defaults?: boolean;
}

export function Workspace({ conn, path, change, testId = "start-workspace", defaults = false }: WorkspaceProps) {
  const app = useApp();
  const initialized = useRef(path !== "" || !defaults);
  const [problem, setProblem] = useState<string>();
  const recent = useReread(conn, "start:workspaces", { cmd: "list_workspaces" }, ["workspaces"]);
  const home = useReread(conn, "start:home", { cmd: "complete_path", prefix: "~/", kind: "directory", limit: 50 }, ["path_completions"]);
  const workspaces = records<aether.Workspace>(recent?.value);
  const initial = defaultWorkspace(workspaces, (home?.value ?? undefined) as aether.PathCompletions | undefined);
  const disabled = usePermission(conn, { cmd: "validate_workspace", path }).refusal !== null;
  const { completion, validation, completionError } = useWorkspaceReads(conn, path, disabled);
  const valid = workspaceValidation(validation?.error ? undefined : validation?.value);
  useEffect(() => {
    if (initialized.current || !recent?.issued || initial === "") return;
    initialized.current = true;
    change(initial);
  }, [initial, recent?.issued, change]);
  const edit = (next: string) => { initialized.current = true; setProblem(undefined); change(next); };
  const browse = async () => {
    try { const picked = await app.host.desktop?.pickDirectory("workspace"); if (picked) edit(picked); }
    catch (error) { setProblem(error instanceof Error ? error.message : String(error)); }
  };
  const paths = [...workspaces.filter(w => w.exists).map(w => w.path), ...(completion?.entries ?? []).filter(p => p.is_dir).map(p => p.path)];
  const suggestions = [...new Set(paths)].filter(p => p.toLowerCase().includes(path.toLowerCase())).map(id => ({ id, label: id }));
  return <div className="b3-stack"><Combo label="Workspace" data-testid={testId} value={path} disabled={disabled} onChange={edit}
    suggestions={suggestions} hint="Choose the folder the agent works in" error={problem ?? validation?.error ?? (valid?.valid === false ? valid.reason ?? "Workspace is unavailable" : undefined)}
    trailing={app.host.desktop ? <Button data-testid={`${testId}-browse`} disabled={disabled} onClick={() => { void browse(); }}>Browse</Button> : undefined} />
    <ReadNote entry={recent} retry={() => app.reread(conn, "start:workspaces", { cmd: "list_workspaces" }, ["workspaces"])} />
    {completionError ? <p role="alert">Path suggestions: {completionError}</p> : null}
    {home?.error && workspaces.length === 0 ? <p role="alert">Home folders: {home.error}</p> : null}
    {path.startsWith("/") && !valid && !validation?.error ? <p role="status">Validating…</p> : null}
    {valid?.valid && valid.path ? <GitPreview conn={conn} path={valid.path} /> : null}
  </div>;
}

function useWorkspaceReads(conn: string, path: string, disabled: boolean) {
  const app = useApp();
  const validationKey = `start:validation:${path}`;
  const completionKey = `start:completion:${path}`;
  const validation = useSelector(selectRead(conn, validationKey));
  const completion = useSelector(selectRead(conn, completionKey));
  useEffect(() => {
    if (disabled || path === "") return;
    const timer = setTimeout(() => {
      void app.reread(conn, completionKey, { cmd: "complete_path", prefix: path, kind: "directory", limit: 50 }, ["path_completions"]);
      if (path.startsWith("/")) void app.reread(conn, validationKey, { cmd: "validate_workspace", path }, ["workspace_validation"]);
    }, 250);
    return () => clearTimeout(timer);
  }, [app, conn, path, disabled, completionKey, validationKey]);
  return { validation, completion: completion?.value as aether.PathCompletions | undefined, completionError: completion?.error };
}

function GitPreview({ conn, path }: { conn: string; path: string }) {
  const app = useApp();
  const command = { cmd: "get_git_summary", workspace: path };
  const permission = usePermission(conn, command);
  const key = `start:git:${path}`;
  const entry = useReread(conn, key, permission.refusal === null ? command : null, ["git_summary"]);
  const summary = entry?.value as aether.GitSummary | undefined;
  return <div><ReadNote entry={entry} retry={() => app.reread(conn, key, command, ["git_summary"])} />
    <p role="status">Ready{summary?.repository ? ` · ${summary.branch ?? "Detached HEAD"} · ${summary.dirty ? "uncommitted changes" : "clean"}` : ""}</p></div>;
}
