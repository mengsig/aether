// `#/learn/graph`: the knowledge graph, coloured by mastery, as a drawing or a list, with a panel for
// the chosen proposition. It stays current while extraction jobs finish.

import { layeredLayout } from "@aether-ui/graph";
import { Button, EmptyState, ErrorState } from "@aether-ui/ui";
import { useMemo, useState, type FormEvent } from "react";
import { useLearn, useNoesisRead } from "./data.ts";
import { useGraphFreshness } from "./graph-jobs.ts";
import { knowledgeGraph, knowledgeModel, masteryByProposition } from "./graph-model.ts";
import { GraphView, PropositionList, type LayoutFn } from "./GraphView.tsx";
import { LearnFrame, type OpenGate } from "./LearnFrame.tsx";
import { MasteryBadge } from "./Mastery.tsx";
import { MASTERY_ORDER } from "./model.ts";
import { PropositionPanel } from "./PropositionPanel.tsx";
import { learnerConcepts } from "./UnderstandingScreen.tsx";

type Shown = "graph" | "list";

export function GraphScreen({ conn, layout = layeredLayout }: OpenGate & { layout?: LayoutFn }) {
  const [query, setQuery] = useState("");
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
        <div className="learn-segmented" role="group" aria-label="Show as">
          {(["graph", "list"] as const).map((view) => <Button key={view} size="sm" variant="quiet" aria-pressed={shown === view} onClick={() => setShown(view)}>{view === "graph" ? "Graph" : "List"}</Button>)}
        </div>
        <ul className="learn-legend" aria-label="Mastery colours">
          {MASTERY_ORDER.map((state) => <li key={state}><MasteryBadge state={state} /></li>)}
        </ul>
      </form>
      {graph.truncated && <p className="learn-muted">Showing part of a large graph. Search to narrow it.</p>}
      {graph.nodes.length === 0
        ? <EmptyState title={search === "" ? "The graph is empty" : "Nothing matches"}>{search === "" ? "Noesis adds true statements here when you add a session to the knowledge graph." : "Try other words."}</EmptyState>
        : <div className="learn-graph-main">
            {shown === "graph"
              ? <GraphView model={model} layout={layout} selected={selected} onSelect={setSelected} />
              : <PropositionList nodes={graph.nodes} mastery={mastery} selected={selected} onSelect={setSelected} />}
            {chosen && <PropositionPanel conn={conn} proposition={chosen} mastery={mastery.get(chosen.id) ?? "unseen"} onClose={() => setSelected(undefined)} />}
          </div>}
    </div>
  );
}

export default function GraphRoute() {
  return <LearnFrame view="graph" title="Knowledge">{(gate) => <GraphScreen {...gate} />}</LearnFrame>;
}
