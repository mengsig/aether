import { useId, useState } from "react";
import { selectRead, useApp, usePermission, useReread, useSelector, type ReadEntry } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Field } from "@aether-ui/ui";
import { Action } from "../controls/kit.tsx";
import { ReadNote } from "../memory/shared.tsx";
import { invariant } from "../fleet/invariant.ts";
import { recentWorkspaces, splitPath, workspaceValidation } from "./data.ts";

interface WorkspaceProps {
  readonly conn: string;
  /** The folder in effect: the user's pick, else the most recent workspace, "" when there is none. */
  readonly path: string;
  readonly change: (path: string) => void;
  readonly recent: ReadEntry | undefined;
}

export function Workspace({ conn, path, change, recent }: WorkspaceProps) {
  const app = useApp();
  const name = useId();
  const [other, setOther] = useState(false);
  const [typed, setTyped] = useState("");
  const folders = recentWorkspaces(recent?.value);
  const validate = { cmd: "validate_workspace", path };
  const disabled = usePermission(conn, validate).refusal !== null;
  const validationKey = `start:validation:${path}`;
  const validation = useReread(conn, validationKey, !disabled && path.startsWith("/") ? validate : null, ["workspace_validation"]);
  const valid = workspaceValidation(validation?.value);
  const pick = (folder: string) => { setOther(false); change(folder); };
  const chooseOther = () => { setOther(true); change(typed.trim()); };
  return <fieldset className="start-section"><legend className="start-section-title">Workspace</legend>
    <ReadNote entry={recent} retry={() => app.reread(conn, "start:workspaces", { cmd: "list_workspaces" }, ["workspaces"])} />
    <div role="radiogroup" aria-label="Workspace" className="start-section">
      {folders.map(w => <FolderChoice key={w.path} group={name} path={w.path} checked={!other && path === w.path} disabled={disabled} onChoose={() => pick(w.path)} />)}
      <label className="start-choice" data-disabled={disabled ? "" : undefined}>
        <input type="radio" className="start-choice-radio" name={name} checked={other} disabled={disabled} onChange={chooseOther} />
        <span className="start-choice-name">Other folder</span></label>
    </div>
    {other && <OtherFolder conn={conn} typed={typed} disabled={disabled} setTyped={setTyped} commit={value => change(value.trim())} />}
    {validation && <ReadNote entry={validation} retry={() => app.reread(conn, validationKey, validate, ["workspace_validation"])} />}
    {valid && !valid.valid && <p role="status" className="b3-note" data-state="failed">{valid.reason}</p>}
    {valid?.valid && valid.path && <GitPreview conn={conn} path={valid.path} />}
  </fieldset>;
}

function FolderChoice({ group, path, checked, disabled, onChoose }: {
  readonly group: string; readonly path: string; readonly checked: boolean; readonly disabled: boolean; readonly onChoose: () => void;
}) {
  const { name, parent } = splitPath(path);
  invariant(name.length > 0, "a folder choice shows a name");
  return <label className="start-choice" title={path} data-disabled={disabled ? "" : undefined}>
    <input type="radio" className="start-choice-radio" name={group} checked={checked} disabled={disabled} onChange={onChoose} />
    <span className="start-choice-name">{name}</span>
    <span className="start-choice-path" title={path}>{parent || "/"}</span></label>;
}

function OtherFolder({ conn, typed, disabled, setTyped, commit }: {
  readonly conn: string; readonly typed: string; readonly disabled: boolean; readonly setTyped: (value: string) => void; readonly commit: (value: string) => void;
}) {
  const app = useApp();
  const completionKey = `start:completion:${typed}`;
  const completion = useSelector(selectRead(conn, completionKey));
  const complete = { cmd: "complete_path", prefix: typed, kind: "directory", limit: 50 };
  const completions = completion?.value as aether.PathCompletions | undefined;
  const absolute = typed.trim().startsWith("/");
  const choose = (path: string) => { setTyped(path); commit(path); };
  return <div className="start-section">
    <Field label="Folder path" hint="An absolute path, such as /home/you/project" value={typed} disabled={disabled}
      {...(typed !== "" && !absolute ? { error: "Start the path with /" } : {})}
      onChange={e => setTyped(e.target.value)} onBlur={() => commit(typed)} onKeyDown={e => { if (e.key === "Enter") commit(typed); }} />
    <Action conn={conn} command={complete} label="Complete path" blocked={absolute ? null : "Type an absolute path first"}
      onRun={() => app.reread(conn, completionKey, complete, ["path_completions"])} />
    {completion && <ReadNote entry={completion} retry={() => app.reread(conn, completionKey, complete, ["path_completions"])} />}
    {completions?.entries.map(p => <FolderChoice key={p.path} group={`${completionKey}`} path={p.path} checked={false} disabled={disabled} onChoose={() => choose(p.path)} />)}
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
    {summary && <p className="b3-note" data-state="done">{summary.repository ? `${summary.branch ?? "Detached HEAD"} · ${summary.dirty ? "Uncommitted changes" : "Clean"}` : "Not a Git repository"}</p>}</div>;
}
