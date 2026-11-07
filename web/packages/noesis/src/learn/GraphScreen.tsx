// `#/learn/graph`: the knowledge graph in a force layout, coloured by mastery, with a panel for the
// chosen proposition. Deleting a proposition or a source asks first.

import { routeHref, useDraft } from "@aether-ui/core";
import { forceLayout, GraphCanvas } from "@aether-ui/graph";
import type { aether, GraphModel, Layout } from "@aether-ui/protocol";
import { Button, Dialog, EmptyState, ErrorState, Field, Segmented } from "@aether-ui/ui";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import { errorText, useLearn, useNoesisChange, useNoesisRead, useOperateRefusal } from "./data.ts";
import { knowledgeGraph, knowledgeModel, masteryByProposition, spreadForLabels, structureKey } from "./graph-model.ts";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { MasteryBadge } from "./Mastery.tsx";
import { draftKey, invariant, MASTERY_ORDER, type Mastery } from "./model.ts";
import { learnerConcepts } from "./UnderstandingScreen.tsx";

export type LayoutFn = (model: GraphModel) => Promise<Layout>;

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

const EMPTY_SEARCH: { readonly query: string } = { query: "" };

export function GraphScreen({ conn, layout = forceLayout }: OpenGate & { layout?: LayoutFn }) {
  const draft = useDraft(draftKey("graph-search", conn), EMPTY_SEARCH);
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
    setSearch(draft.value.query.trim());
  };
  const showAll = () => {
    draft.clear();
    setSearch("");
  };
  if (graph === undefined || model === undefined) {
    if (read.error !== null) return <ErrorState title="Could not read the knowledge graph" message={read.error} onRetry={read.reload} />;
    return <p className="learn-muted" aria-busy="true">Reading the knowledge graph…</p>;
  }
  if (graph.nodes.length === 0 && search === "") return <EmptyGraph />;
  const chosen = graph.nodes.find((node) => node.id === selected);
  return (
    <div className="learn-graph">
      <form className="learn-graph-bar" role="search" onSubmit={submit}>
        <Field className="learn-search" type="search" label="Search the knowledge graph" data-testid="learn-graph-search" placeholder="Search statements"
          value={draft.value.query} onChange={(e) => draft.update({ query: e.target.value })} />
        <Button type="submit">Search</Button>
        {search !== "" && <Button variant="quiet" onClick={showAll}>Show all</Button>}
      </form>
      <MasteryLegend />
      {graph.truncated && <p className="learn-muted">Showing part of a large graph. Search to narrow it.</p>}
      {graph.nodes.length === 0
        ? <EmptyState title="Nothing matches" action={<Button onClick={showAll}>Show all</Button>}>No statement uses those words. Try other words.</EmptyState>
        : <div className="learn-graph-main">
            <GraphView model={model} layout={layout} selected={selected} onSelect={setSelected} />
            {chosen
              ? <PropositionPanel conn={conn} proposition={chosen} mastery={mastery.get(chosen.id) ?? "unseen"} onClose={() => setSelected(undefined)} />
              : <p className="learn-graph-tip learn-muted">Choose a statement to see where it came from and how well you know it.</p>}
          </div>}
    </div>
  );
}

/** What appears here and how it gets here, with the one step that fills it. */
function EmptyGraph() {
  return (
    <div data-testid="learn-graph-empty">
      <EmptyState title="Your knowledge graph is empty" action={<a className="ui-button" data-variant="primary" data-size="md" href="#/learn">Start a session</a>}>
        <p>True statements you learn appear here, linked by what follows from what, each coloured by how well you know it.</p>
        <p>Start a session, then choose “Add to knowledge graph” from it.</p>
      </EmptyState>
    </div>
  );
}

/** The mastery tones the canvas draws, each with its mark and words so colour is never the only signal. */
function MasteryLegend() {
  return (
    <div className="learn-legend-row">
      <ul className="learn-legend" aria-label="Mastery colours" data-testid="learn-graph-legend">
        {MASTERY_ORDER.map((state) => <li key={state}><MasteryBadge state={state} /></li>)}
      </ul>
      <p className="learn-muted learn-graph-keys">Arrow keys move between statements · Enter opens one · + and − zoom · 0 fits the graph</p>
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
      (next) => { if (live) { setPlaced({ key, layout: spreadForLabels(next) }); setFailure(undefined); } },
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
  const [sourceGraph, setSourceGraph] = useState<SourceGraph>("preserve");
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
          <div data-testid="learn-source-graph">
            <Segmented label="Statements it added" layout="cards" options={SOURCE_GRAPH} value={sourceGraph} onChange={(id) => setSourceGraph(sourceGraphOf(id))} />
          </div>
        )}
        <p className="learn-muted">What Noesis knows about your understanding stays as it is.</p>
      </Dialog>
    </aside>
  );
}

export default function GraphRoute() {
  return <LearnFrame view="graph" title="Knowledge">{(gate) => <GraphScreen {...gate} />}</LearnFrame>;
}
