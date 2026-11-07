import { useMemo, useState } from "react";
import { useApp, useReadPermission, useReread } from "@aether-ui/core";
import type { aether, RouteProps } from "@aether-ui/protocol";
import { Button, EmptyState, Field, Tabs } from "@aether-ui/ui";
import { invariant } from "../fleet/invariant.ts";
import { Section, StandingNotice } from "../controls/kit.tsx";
import { CreateMemory, kinds } from "./CreateMemory.tsx";
import { MemoryRow } from "./MemoryRow.tsx";
import { AgentPicker, records, ReadNote } from "./shared.tsx";
import { V2Sections } from "./V2Sections.tsx";
import "./memory.css";
const proposed = { cmd: "memories", query: { proposed: true } };
export default function MemoryScreen({ params }: RouteProps) {
  const conn = params.conn ?? "";
  const app = useApp();
  const [text, setText] = useState("");
  const [scope, setScope] = useState("all");
  const [repository, setRepository] = useState("");
  const [agent, setAgent] = useState("");
  const [kind, setKind] = useState("all");
  const [forgotten, setForgotten] = useState(false);
  const [open, setOpen] = useState(false);
  const search = text.trim();
  // A blank text must be omitted: the store orders by FTS rank whenever text is present.
  const command = { cmd: "memories", query: { ...(search === "" ? {} : { text: search }), limit: 100, include_forgotten: forgotten, include_superseded: true,
    kinds: kind === "all" ? [] : [kind], scope: scopeFilter(scope, repository, agent) } };
  const entry = useReread(conn, "memory:list", command, ["memories"]);
  const permission = useReadPermission(conn, proposed);
  const proposals = useReread(conn, "memory:proposals", permission.refusal === null ? proposed : null, ["memories"]);
  const refresh = async () => {
    await app.reread(conn, "memory:list", command, ["memories"]);
    if (permission.refusal === null) await app.reread(conn, "memory:proposals", proposed, ["memories"]);
  };
  const rows = records<aether.MemoryRecord>(entry?.value);
  const known = useMemo(() => new Map(rows.map(memory => [memory.id, memory.text])), [rows]);
  return <div className="ui-page b3-screen mem-screen"><header className="ui-page-head b3-screen-head"><h1 className="ui-visually-hidden b3-screen-title">Memory</h1>
    <p className="ui-page-lede b3-screen-sub">Review what stays with your agents.</p><Button onClick={() => setOpen(true)}>Create memory</Button></header>
    <StandingNotice conn={conn} />
    <Section title="Memories"><Field label="Search memories" value={text} onChange={e => setText(e.target.value)} />
      <Tabs label="Scope" selected={scope} onSelect={setScope} tabs={[{ id: "all", label: "All scopes", panel: null }, { id: "global", label: "Global", panel: null }, { id: "repository", label: "Repository", panel: <Field label="Repository filter" value={repository} onChange={e => setRepository(e.target.value)} /> },
        { id: "agent", label: "Agent", panel: <AgentPicker conn={conn} label="Agent filter" value={agent} onChange={setAgent} /> }]} />
      <Tabs label="Kind" selected={kind} onSelect={setKind} tabs={["all", ...kinds].map(id => ({ id, label: id, panel: null }))} />
      <label className="b3-check"><input type="checkbox" checked={forgotten} onChange={e => setForgotten(e.target.checked)} /><span>Include forgotten</span></label>
      <ReadNote entry={entry} retry={refresh} />
      {entry?.issued && !rows.length ? <EmptyState title="No memories yet" /> : null}
      {rows.map(memory => <MemoryRow key={memory.id} conn={conn} memory={memory} refresh={refresh} />)}
    </Section>
    <Section title="Proposals" hint="Host only · Review proposed memories before accepting them.">
      {permission.refusal ? <p role="note" className="b3-note">{permission.refusal}</p> : <><ReadNote entry={proposals} retry={refresh} />
        {records<aether.MemoryRecord>(proposals?.value).map(memory => <MemoryRow key={memory.id} conn={conn} memory={memory} proposal refresh={refresh} />)}
        {proposals?.issued && records(proposals.value).length === 0 ? <EmptyState title="No proposals to review" /> : null}</>}
    </Section><V2Sections conn={conn} known={known} refresh={refresh} /><CreateMemory conn={conn} open={open} close={() => setOpen(false)} refresh={refresh} />
  </div>;
}

function scopeFilter(scope: string, repository: string, agent: string): aether.MemoryScope | null {
  invariant(["all", "global", "repository", "agent"].includes(scope), "the scope tab is one the screen offers");
  invariant(typeof repository === "string" && typeof agent === "string", "filter drafts are strings");
  if (scope === "all") return null;
  if (scope === "global") return { scope: "global" };
  return scope === "repository" ? { scope: "repository", id: repository } : { scope: "agent", id: agent };
}
