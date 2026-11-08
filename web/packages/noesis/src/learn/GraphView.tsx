// The knowledge graph drawn left to right, from what a statement rests on to what it supports, with
// zoom and fit buttons, or the same statements as a plain list.

import { GraphCanvas } from "@aether-ui/graph";
import type { aether, GraphModel, Layout } from "@aether-ui/protocol";
import { Button, ErrorState } from "@aether-ui/ui";
import { useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "./data.ts";
import { readableTitle, structureKey } from "./graph-model.ts";
import { MasteryBadge } from "./Mastery.tsx";
import { invariant, type Mastery } from "./model.ts";

export type LayoutFn = (model: GraphModel) => Promise<Layout>;

/** The canvas's own keys: `+` and `-` zoom about the centre, `0` fits the whole graph. */
const CONTROLS = [{ key: "+", label: "Zoom in" }, { key: "-", label: "Zoom out" }, { key: "0", label: "Fit" }] as const;

export function GraphView({ model, layout, selected, onSelect }: { model: GraphModel; layout: LayoutFn; selected: string | undefined; onSelect: (id: string) => void }) {
  const key = useMemo(() => structureKey(model), [model]);
  const [placed, setPlaced] = useState<{ key: string; layout: Layout } | undefined>();
  const [failure, setFailure] = useState<string | undefined>();
  const [attempt, setAttempt] = useState(0);
  const frame = useRef<HTMLDivElement>(null);
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
  // The buttons press the canvas's documented keys, so they and the keyboard move the same camera.
  const press = (keyName: string) => {
    const canvas = frame.current?.querySelector("canvas");
    invariant(canvas != null, "the zoom buttons sit beside a drawn canvas");
    canvas.dispatchEvent(new KeyboardEvent("keydown", { key: keyName, bubbles: true }));
  };
  if (failure !== undefined) return <ErrorState title="The graph could not be laid out" message={failure} onRetry={() => setAttempt((n) => n + 1)} />;
  return (
    <div className="learn-graph-frame">
      <div className="learn-graph-tools" role="toolbar" aria-label="Graph view">
        {CONTROLS.map((control) => <Button key={control.key} size="sm" variant="quiet" aria-keyshortcuts={control.key} onClick={() => press(control.key)}>{control.label}</Button>)}
        <span className="learn-muted">Arrows run left to right: from what a statement rests on to what it supports.</span>
      </div>
      <div ref={frame} className="learn-graph-canvas" data-settled={placed?.key === key || undefined}>
        <GraphCanvas model={model} layout={placed?.key === key ? placed.layout : undefined} label="Knowledge graph" {...(selected === undefined ? {} : { selected })} onActivate={onSelect} />
      </div>
    </div>
  );
}

/** The statements as a list, by title: readable at any size and with any number of them. */
export function PropositionList({ nodes, mastery, selected, onSelect }: {
  nodes: readonly aether.Proposition[]; mastery: ReadonlyMap<string, Mastery>; selected: string | undefined; onSelect: (id: string) => void;
}) {
  const rows = useMemo(() => nodes.map((node) => ({ node, title: readableTitle(node) })).sort((a, b) => a.title.localeCompare(b.title)), [nodes]);
  invariant(rows.length === nodes.length, "every statement is listed once");
  return (
    <ul className="learn-prop-list" aria-label="Statements">
      {rows.map(({ node, title }) => (
        <li key={node.id}>
          <button type="button" className="learn-prop-row" aria-current={selected === node.id || undefined} onClick={() => onSelect(node.id)}>
            <span className="learn-prop-title">{title}</span>
            <MasteryBadge state={mastery.get(node.id) ?? "unseen"} />
            <span className="learn-muted learn-prop-statement">{node.statement}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}
