import { useState } from "react";
import { Field, Sheet, Tabs } from "@aether-ui/ui";
import { usePermission } from "@aether-ui/core";
import { Action, RunNote, useRun } from "../controls/kit.tsx";
export const kinds = ["fact", "decision", "lesson", "constraint", "preference"];
export function CreateMemory({ conn, open, close, refresh }: { conn: string; open: boolean; close: () => void; refresh: () => Promise<unknown> }) {
  const [text, setText] = useState("");
  const [kind, setKind] = useState("fact");
  const [repository, setRepository] = useState("");
  const [tags, setTags] = useState("");
  const run = useRun(conn);
  const command = { cmd: "create_memory", spec: { text: text.trim(), kind, scope: repository.trim() ? { scope: "repository", id: repository.trim() } : { scope: "global" }, tags: tags.split(",").map(t => t.trim()).filter(Boolean) } };
  const disabled = usePermission(conn, command).refusal !== null;
  return <Sheet open={open} onClose={close} title="Create memory"><div className="b3-stack">
    <Field label="Memory text" value={text} onChange={e => setText(e.target.value)} disabled={disabled} />
    <Tabs label="Memory kind" selected={kind} onSelect={setKind} tabs={kinds.map(id => ({ id, label: id, panel: null }))} />
    <Field label="Repository root" hint="Leave blank for Global" value={repository} onChange={e => setRepository(e.target.value)} disabled={disabled} />
    <Field label="Tags" hint="Comma separated" value={tags} onChange={e => setTags(e.target.value)} disabled={disabled} />
    <Action conn={conn} command={command} label="Save memory" blocked={text.trim() ? null : "Enter memory text"} busy={run.status.state === "busy"}
      onRun={async () => { if (await run.send(command)) { await refresh(); close(); } }} />
    <RunNote status={run.status} />
  </div></Sheet>;
}
