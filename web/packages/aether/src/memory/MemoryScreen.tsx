import { useState } from "react";
import { useApp, usePermission, useReread } from "@aether-ui/core";
import type { aether, RouteProps } from "@aether-ui/protocol";
import { Button, EmptyState, Field, Tabs } from "@aether-ui/ui";
import { Section, StandingNotice } from "../controls/kit.tsx";
import { CreateMemory, kinds } from "./CreateMemory.tsx";
import { MemoryRow } from "./MemoryRow.tsx";
import { records, ReadNote } from "./shared.tsx";
const proposed = { cmd: "memories", query: { proposed: true } };
export default function MemoryScreen({ params }: RouteProps) {
  const conn = params.conn ?? "";
  const app = useApp();
  const [text, setText] = useState("");
  const [scope, setScope] = useState("all");
  const [repository, setRepository] = useState("");
  const [kind, setKind] = useState("all");
  const [forgotten, setForgotten] = useState(false);
  const [open, setOpen] = useState(false);
  const command = { cmd: "memories", query: { text, limit: 100, include_forgotten: forgotten, include_superseded: true,
    kinds: kind === "all" ? [] : [kind], scope: scope === "all" ? null : scope === "global" ? { scope } : { scope, id: repository } } };
  const entry = useReread(conn, "memory:list", command, ["memories"]);
  const permission = usePermission(conn, proposed);
  const proposals = useReread(conn, "memory:proposals", permission.refusal === null ? proposed : null, ["memories"]);
  const refresh = async () => {
    await app.reread(conn, "memory:list", command, ["memories"]);
    if (permission.refusal === null) await app.reread(conn, "memory:proposals", proposed, ["memories"]);
  };
  const rows = records<aether.MemoryRecord>(entry?.value);
  return <div className="b3-screen"><header className="b3-screen-head"><h1 className="b3-screen-title">Memory</h1>
    <p className="b3-screen-sub">Review what stays with your agents.</p><Button onClick={() => setOpen(true)}>Create memory</Button></header>
    <StandingNotice conn={conn} />
    <Section title="Memories"><Field label="Search memories" value={text} onChange={e => setText(e.target.value)} />
      <Tabs label="Scope" selected={scope} onSelect={setScope} tabs={[{ id: "all", label: "All scopes", panel: null }, { id: "global", label: "Global", panel: null }, { id: "repository", label: "Repository", panel: <Field label="Repository filter" value={repository} onChange={e => setRepository(e.target.value)} /> }]} />
      <Tabs label="Kind" selected={kind} onSelect={setKind} tabs={["all", ...kinds].map(id => ({ id, label: id, panel: null }))} />
      <label className="b3-check"><input type="checkbox" checked={forgotten} onChange={e => setForgotten(e.target.checked)} /><span>Include forgotten</span></label>
      <ReadNote entry={entry} retry={refresh} />
      {entry?.issued && !rows.length ? <EmptyState title="No memories yet" /> : null}
      {rows.map(memory => <MemoryRow key={memory.id} conn={conn} memory={memory} refresh={refresh} />)}
    </Section>
    <Section title="Proposals" hint="Host only · Review proposed memories before accepting them.">
      {permission.refusal ? <p role="note">{permission.refusal}</p> : <><ReadNote entry={proposals} retry={refresh} />
        {records<aether.MemoryRecord>(proposals?.value).map(memory => <MemoryRow key={memory.id} conn={conn} memory={memory} proposal refresh={refresh} />)}
        {proposals?.issued && records(proposals.value).length === 0 ? <EmptyState title="No proposals to review" /> : null}</>}
    </Section><CreateMemory conn={conn} open={open} close={() => setOpen(false)} refresh={refresh} />
  </div>;
}
