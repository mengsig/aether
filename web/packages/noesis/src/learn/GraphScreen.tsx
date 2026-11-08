// `#/learn/graph`: the knowledge graph, coloured by mastery, as a drawing or a list, with a panel for
// the chosen proposition. It stays current while extraction jobs finish.

import { useDraft } from "@aether-ui/core";
import { layeredLayout } from "@aether-ui/graph";
import { Button, EmptyState, ErrorState, Field, Segmented } from "@aether-ui/ui";
import { useMemo, useState, type FormEvent } from "react";
import { useLearn, useNoesisRead } from "./data.ts";
import { useGraphFreshness } from "./graph-jobs.ts";
import { knowledgeGraph, knowledgeModel, masteryByProposition } from "./graph-model.ts";
import { GraphView, PropositionList, type LayoutFn } from "./GraphView.tsx";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { MasteryBadge } from "./Mastery.tsx";
import { draftKey, invariant, MASTERY_ORDER } from "./model.ts";
import { PropositionPanel } from "./PropositionPanel.tsx";
import { learnerConcepts } from "./UnderstandingScreen.tsx";

const SHOWN = [{ id: "graph", label: "Graph" }, { id: "list", label: "List" }] as const;
type Shown = (typeof SHOWN)[number]["id"];

function shownOf(id: string): Shown {
  const found = SHOWN.find((option) => option.id === id);
  invariant(found !== undefined, `${id} is a way to show the graph`);
  return found.id;
}

const EMPTY_SEARCH: { readonly query: string } = { query: "" };

export function GraphScreen({ conn, layout = layeredLayout }: OpenGate & { layout?: LayoutFn }) {
  const draft = useDraft(draftKey("graph-search", conn), EMPTY_SEARCH);
  const [search, setSearch] = useState("");
  const [shown, setShown] = useState<Shown>("graph");
  const read = useNoesisRead(conn, search === "" ? { kind: "graph" } : { kind: "graph", query: search });
  useGraphFreshness(conn, read.reload);
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
        <Segmented label="Show as" options={SHOWN} value={shown} onChange={(id) => setShown(shownOf(id))} />
      </form>
      <MasteryLegend />
      {graph.truncated && <p className="learn-muted">Showing part of a large graph. Search to narrow it.</p>}
      {graph.nodes.length === 0
        ? <EmptyState title="Nothing matches" action={<Button onClick={showAll}>Show all</Button>}>No statement uses those words. Try other words.</EmptyState>
        : <div className="learn-graph-main">
            {shown === "graph"
              ? <GraphView model={model} layout={layout} selected={selected} onSelect={setSelected} />
              : <PropositionList nodes={graph.nodes} mastery={mastery} selected={selected} onSelect={setSelected} />}
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

export default function GraphRoute() {
  return <LearnFrame view="graph" title="Knowledge">{(gate) => <GraphScreen {...gate} />}</LearnFrame>;
}
