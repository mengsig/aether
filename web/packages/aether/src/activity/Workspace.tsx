import { useApp, usePermission, useReread, useSelector, selectRead } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, Field } from "@aether-ui/ui";
import { Action } from "../controls/kit.tsx";
import { workspaceValidation } from "./data.ts";
import { records, ReadNote } from "../memory/shared.tsx";

export function Workspace({ conn, path, change }: { conn: string; path: string; change: (path: string) => void }) {
  const app = useApp();
  const recent = useReread(conn, "start:workspaces", { cmd: "list_workspaces" }, ["workspaces"]);
  const validationKey = `start:validation:${path}`;
  const completionKey = `start:completion:${path}`;
  const validation = useSelector(selectRead(conn, validationKey));
  const completion = useSelector(selectRead(conn, completionKey));
  const valid = workspaceValidation(validation?.value);
  const validate = { cmd: "validate_workspace", path };
  const complete = { cmd: "complete_path", prefix: path, kind: "directory", limit: 50 };
  const disabled = usePermission(conn, validate).refusal !== null;
  const completions = completion?.value as aether.PathCompletions | undefined;
  return <div className="b3-stack"><Field label="Workspace" value={path} disabled={disabled} onChange={e => change(e.target.value)} />
    <ReadNote entry={recent} retry={() => app.reread(conn, "start:workspaces", { cmd: "list_workspaces" }, ["workspaces"])} />
    <div className="b3-row">{records<{ path: string; exists: boolean }>(recent?.value).map(w => <Button key={w.path} disabled={disabled || !w.exists} onClick={() => change(w.path)}>{w.path}</Button>)}</div>
    <div className="b3-row"><Action conn={conn} command={complete} label="Complete path" onRun={() => app.reread(conn, completionKey, complete, ["path_completions"])} />
      <Action conn={conn} command={validate} label="Validate workspace" blocked={path.startsWith("/") ? null : "Choose an absolute workspace path"}
        onRun={() => app.reread(conn, validationKey, validate, ["workspace_validation"])} /></div>
    {completion && <><ReadNote entry={completion} retry={() => app.reread(conn, completionKey, complete, ["path_completions"])} />
      {completions?.entries.map(p => <Button key={p.path} disabled={disabled} onClick={() => change(p.path)}>{p.path}</Button>)}</>}
    {validation && <><ReadNote entry={validation} retry={() => app.reread(conn, validationKey, validate, ["workspace_validation"])} />
      <p role="status">{valid?.valid ? `Workspace ready: ${valid.path}` : valid?.reason}</p></>}
    {valid?.valid && valid.path && <GitPreview conn={conn} path={valid.path} />}
  </div>;
}

function GitPreview({ conn, path }: { conn: string; path: string }) {
  const app = useApp();
  const command = { cmd: "get_git_summary", workspace: path };
  const permission = usePermission(conn, command);
  const key = `start:git:${path}`;
  const entry = useReread(conn, key, permission.refusal === null ? command : null, ["git_summary"]);
  const summary = entry?.value as aether.GitSummary | undefined;
  return <div><ReadNote entry={entry} retry={() => app.reread(conn, key, command, ["git_summary"])} />
    {summary && <p>{summary.repository ? `${summary.branch ?? "Detached HEAD"} · ${summary.dirty ? "Uncommitted changes" : "Clean"}` : "Not a Git repository"}</p>}</div>;
}
