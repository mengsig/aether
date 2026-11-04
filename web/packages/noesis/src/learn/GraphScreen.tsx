// `#/learn/graph`: the knowledge graph in a force layout, coloured by mastery, with a panel for the
// chosen proposition. Deleting a proposition or a source asks first.

import { routeHref } from "@aether-ui/core";
import { forceLayout, GraphCanvas } from "@aether-ui/graph";
import type { aether, GraphModel, Layout } from "@aether-ui/protocol";
import { Button, Dialog, EmptyState, ErrorState } from "@aether-ui/ui";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import { errorText, useLearn, useNoesisChange, useNoesisRead, useOperateRefusal } from "./data.ts";
import { knowledgeGraph, knowledgeModel, masteryByProposition, structureKey } from "./graph-model.ts";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { MasteryBadge } from "./Mastery.tsx";
import { MASTERY_ORDER, type Mastery } from "./model.ts";
import { learnerConcepts } from "./UnderstandingScreen.tsx";

export type LayoutFn = (model: GraphModel) => Promise<Layout>;

type Pending = { readonly kind: "proposition"; readonly id: string } | { readonly kind: "source"; readonly id: string };

export function GraphScreen({ conn, layout = forceLayout }: OpenGate & { layout?: LayoutFn }) {
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const read = useNoesisRead(conn, search === "" ? { kind: "graph" } : { kind: "graph", query: search });
  useNoesisRead(conn, { kind: "learner" });
  const learn = useLearn(conn);
  const held = learn.reads[search === "" ? "graph" : `graph?${search}`];
  const graph = useMemo(() => knowledgeGraph(held?.value), [held]);
  const learner = learn.reads.learner;
  const mastery = useMemo(() => masteryByProposition(learnerConcepts(learner?.value)), [learner]);
  const model = useMemo(() => (graph === undefined ? undefined : knowledgeModel(graph, mastery)), [graph, mastery]);
  const [selected, setSelected] = useState<string | undefined>();
  const submit = (event: FormEvent) => {
    event.preventDefault();
    setSearch(query.trim());
  };
  if (graph === undefined || model === undefined) {
    if (read.error !== null) return <ErrorState title="Could not read the knowledge graph" message={read.error} onRetry={read.reload} />;
    return <p className="learn-muted" aria-busy="true">Reading the knowledge graph…</p>;
  }
  const chosen = graph.nodes.find((node) => node.id === selected);
  return (
    <div className="learn-graph">
      <form className="learn-graph-bar" role="search" onSubmit={submit}>
        <input className="ui-field-input learn-search" type="search" aria-label="Search the knowledge graph" placeholder="Search statements" value={query} onChange={(e) => setQuery(e.target.value)} />
        <Button type="submit">Search</Button>
        {search !== "" && <Button variant="quiet" onClick={() => { setQuery(""); setSearch(""); }}>Show all</Button>}
        <ul className="learn-legend" aria-label="Mastery colours">
          {MASTERY_ORDER.map((state) => <li key={state}><MasteryBadge state={state} /></li>)}
        </ul>
      </form>
      {graph.truncated && <p className="learn-muted">Showing part of a large graph. Search to narrow it.</p>}
      {graph.nodes.length === 0
        ? <EmptyState title={search === "" ? "The graph is empty" : "Nothing matches"}>{search === "" ? "Noesis adds true statements here when you add a session to the knowledge graph." : "Try other words."}</EmptyState>
        : <div className="learn-graph-main">
            <GraphView model={model} layout={layout} selected={selected} onSelect={setSelected} />
            {chosen && <PropositionPanel conn={conn} proposition={chosen} mastery={mastery.get(chosen.id) ?? "unseen"} onClose={() => setSelected(undefined)} />}
          </div>}
    </div>
  );
}

function GraphView({ model, layout, selected, onSelect }: { model: GraphModel; layout: LayoutFn; selected: string | undefined; onSelect: (id: string) => void }) {
  const key = useMemo(() => structureKey(model), [model]);
  const [placed, setPlaced] = useState<{ key: string; layout: Layout } | undefined>();
  const [failure, setFailure] = useState<string | undefined>();
  const [attempt, setAttempt] = useState(0);
  // The latest model of this structure; a mastery change redraws without a relayout.
  const structural = useMemo(() => model, [key]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    let live = true;
    layout(structural).then(
      (next) => { if (live) { setPlaced({ key, layout: next }); setFailure(undefined); } },
      (error: unknown) => { if (live) setFailure(errorText(error)); },
    );
    return () => { live = false; };
  }, [layout, structural, key, attempt]);
  if (failure !== undefined) return <ErrorState title="The graph could not be laid out" message={failure} onRetry={() => setAttempt((n) => n + 1)} />;
  return (
    <div className="learn-graph-canvas" data-settled={placed?.key === key || undefined}>
      <GraphCanvas model={model} layout={placed?.key === key ? placed.layout : undefined} label="Knowledge graph" {...(selected === undefined ? {} : { selected })} onActivate={onSelect} />
    </div>
  );
}

function PropositionPanel({ conn, proposition, mastery, onClose }: { conn: string; proposition: aether.Proposition; mastery: Mastery; onClose: () => void }) {
  const change = useNoesisChange(conn);
  const refusal = useOperateRefusal(conn);
  const [pending, setPending] = useState<Pending | null>(null);
  const [sourceGraph, setSourceGraph] = useState<"preserve" | "delete_unsupported">("preserve");
  const [failure, setFailure] = useState<string | null>(null);
  const sources = [...new Set(proposition.provenance.map((p) => p.source))];
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
        <h2 className="learn-subtitle">{proposition.name}</h2>
        <Button variant="quiet" size="sm" onClick={onClose}>Close</Button>
      </div>
      <MasteryBadge state={mastery} />
      <p>{proposition.statement}</p>
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
          <fieldset className="learn-fieldset">
            <legend>Statements it added</legend>
            <label className="learn-toggle"><input type="radio" name="source-graph" checked={sourceGraph === "preserve"} onChange={() => setSourceGraph("preserve")} />Keep them in the graph</label>
            <label className="learn-toggle"><input type="radio" name="source-graph" checked={sourceGraph === "delete_unsupported"} onChange={() => setSourceGraph("delete_unsupported")} />Remove those nothing else supports</label>
          </fieldset>
        )}
        <p className="learn-muted">What Noesis knows about your understanding stays as it is.</p>
      </Dialog>
    </aside>
  );
}

export default function GraphRoute() {
  return <LearnFrame view="graph" title="Knowledge">{(gate) => <GraphScreen {...gate} />}</LearnFrame>;
}
