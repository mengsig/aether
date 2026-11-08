// The chosen proposition: its readable title, statement, mastery and sources. Deleting the statement
// or one of its imported sources asks first.

import { routeHref } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, Dialog, Segmented } from "@aether-ui/ui";
import { useState } from "react";
import { useNoesisChange, useOperateRefusal } from "./data.ts";
import { readableTitle } from "./graph-model.ts";
import { MasteryBadge } from "./Mastery.tsx";
import { invariant, type Mastery } from "./model.ts";

export const SOURCE_GRAPH = [
  { id: "preserve", label: "Keep them in the graph", hint: "Statements stay, even with nothing else behind them" },
  { id: "delete_unsupported", label: "Remove those nothing else supports", hint: "Statements another source backs stay" },
] as const;

type SourceGraph = (typeof SOURCE_GRAPH)[number]["id"];

function sourceGraphOf(id: string): SourceGraph {
  const found = SOURCE_GRAPH.find((option) => option.id === id);
  invariant(found !== undefined, `${id} is a source-graph option`);
  return found.id;
}

type Pending = { readonly kind: "proposition"; readonly id: string } | { readonly kind: "source"; readonly id: string };

export function PropositionPanel({ conn, proposition, mastery, onClose }: { conn: string; proposition: aether.Proposition; mastery: Mastery; onClose: () => void }) {
  const change = useNoesisChange(conn);
  const refusal = useOperateRefusal(conn);
  const [pending, setPending] = useState<Pending | null>(null);
  const [sourceGraph, setSourceGraph] = useState<SourceGraph>("preserve");
  const [failure, setFailure] = useState<string | null>(null);
  const sources = [...new Set(proposition.provenance.map((p) => p.source))];
  invariant(proposition.id.length > 0, "the panel shows a stored proposition");
  const confirm = async () => {
    if (pending === null) return;
    const command = pending.kind === "proposition"
      ? { cmd: "noesis_delete_proposition", proposition: pending.id }
      : { cmd: "noesis_delete_source", source: pending.id, graph: sourceGraph };
    const result = await change(command);
    if (!result.ok) return setFailure(result.error);
    setPending(null);
    setFailure(null);
    if (pending.kind === "proposition") onClose();
  };
  return (
    <aside className="learn-panel" aria-label="Proposition">
      <div className="learn-panel-head">
        <h2 className="learn-subtitle">{readableTitle(proposition)}</h2>
        <Button variant="quiet" size="sm" onClick={onClose}>Close</Button>
      </div>
      <MasteryBadge state={mastery} />
      <p className="learn-panel-statement">{proposition.statement}</p>
      {proposition.aliases.length > 0 && <p className="learn-muted">Also called: {proposition.aliases.join(", ")}</p>}
      <h3 className="learn-subtitle">Where it came from</h3>
      <ul className="learn-sources">
        {sources.map((source) => (
          <li key={source}>
            {source.startsWith("ns_") ? <a className="ui-link" href={routeHref("#/learn/sessions/:id", { id: source })}>A session</a> : <span>Imported notes <code>{source}</code></span>}
            {source.startsWith("s_") && <Button size="sm" variant="quiet" disabled={refusal !== null} onClick={() => setPending({ kind: "source", id: source })}>Delete source…</Button>}
          </li>
        ))}
      </ul>
      <Button variant="danger" disabled={refusal !== null} onClick={() => setPending({ kind: "proposition", id: proposition.id })}>Delete statement…</Button>
      {failure !== null && <p className="learn-callout" role="alert">{failure}</p>}
      <Dialog open={pending !== null} onClose={() => setPending(null)} title={pending?.kind === "source" ? "Delete this source?" : "Delete this statement?"}
        description={pending?.kind === "source" ? "Noesis forgets the imported notes." : "The statement leaves the graph, with every chain that uses it."}
        footer={<><Button onClick={() => setPending(null)} data-autofocus>Keep it</Button><Button variant="danger" onClick={() => void confirm()}>Delete</Button></>}>
        {pending?.kind === "source" && (
          <div data-testid="learn-source-graph">
            <Segmented label="Statements it added" layout="cards" options={SOURCE_GRAPH} value={sourceGraph} onChange={(id) => setSourceGraph(sourceGraphOf(id))} />
          </div>
        )}
        <p className="learn-muted">What Noesis knows about your understanding stays as it is.</p>
      </Dialog>
    </aside>
  );
}
