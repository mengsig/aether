import type { aether } from "@aether-ui/protocol";
import { Badge, Button, Field, Sheet, Segmented } from "@aether-ui/ui";
import { connectionDraftKey, useDraft, usePermission } from "@aether-ui/core";
import { Action, RunNote, useRun } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";
import { Repository } from "./Repository.tsx";
import { TextArea } from "@aether-ui/ui";
import { AgentPicker } from "./shared.tsx";
export const kinds = ["fact", "decision", "lesson", "constraint", "preference"];
export function CreateMemory({ conn, open, close, discard, refresh }: { conn: string; open: boolean; close: () => void; discard: () => void; refresh: () => Promise<unknown> }) {
  const draft = useDraft(connectionDraftKey("memory", conn, "create"), { text: "", kind: "fact", target: "global", repository: "", agent: "", tags: "" });
  const { text, kind, target, repository, agent, tags } = draft.value;
  const run = useRun(conn);
  const scope = createScope(target, repository.trim(), agent);
  const command = { cmd: "create_memory", spec: { text: text.trim(), kind, scope: scope ?? { scope: "global" }, tags: tags.split(",").map(t => t.trim()).filter(Boolean) } };
  const disabled = usePermission(conn, command).refusal !== null;
  return <Sheet open={open} onClose={close} title="Create memory"><div className="b3-stack">
    <TextArea label="Memory text" data-testid="memory-create-text" value={text} onChange={e => draft.update({ text: e.target.value })} disabled={disabled} />
    <Segmented label="Memory kind" value={kind} onChange={kind => draft.update({ kind })} options={kinds.map(id => ({ id, label: id }))} />
    <Segmented label="Memory scope" value={target} onChange={target => draft.update({ target })} options={["global", "repository", "agent"].map(id => ({ id, label: id.charAt(0).toUpperCase() + id.slice(1) }))} />
    {target === "repository" && <Repository conn={conn} label="Repository root" testId="memory-create-repository" value={repository} onChange={repository => draft.update({ repository })} disabled={disabled} />}
    {target === "agent" && <AgentPicker conn={conn} label="Agent" value={agent} onChange={agent => draft.update({ agent })} disabled={disabled} />}
    <Field label="Tags" hint="Comma separated" value={tags} onChange={e => draft.update({ tags: e.target.value })} disabled={disabled} />
    <div className="b3-row">{[...new Set(tags.split(",").map(t => t.trim()).filter(Boolean))].map(tag => <Badge key={tag}>{tag}</Badge>)}</div>
    <Action data-testid="memory-create-submit" variant="primary" conn={conn} command={command} label="Save memory" blocked={text.trim() === "" ? "Enter memory text" : scope === null ? SCOPE_BLOCK[target] ?? null : null} busy={run.status.state === "busy"}
      onRun={async () => { if (await run.send(command)) { draft.clearIfUnchanged(); await refresh(); discard(); } }} />
    <RunNote status={run.status} />
    {run.status.state === "failed" && <Button variant="quiet" onClick={() => { draft.clear(); discard(); }}>Discard</Button>}
  </div></Sheet>;
}

const SCOPE_BLOCK: Record<string, string> = { repository: "Choose a repository", agent: "Choose an agent" };

/** The scope to create in, or null while the chosen target lacks its repository or agent. */
function createScope(target: string, repository: string, agent: string): aether.OperatorMemoryScope | null {
  invariant(["global", "repository", "agent"].includes(target), "the scope target is one the form offers");
  invariant(repository === repository.trim(), "the repository id arrives trimmed");
  if (target === "global") return { scope: "global" };
  if (target === "repository") return repository === "" ? null : { scope: "repository", root: repository };
  return agent === "" ? null : { scope: "agent", id: agent };
}
