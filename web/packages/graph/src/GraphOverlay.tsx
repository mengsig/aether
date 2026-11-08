import type { GNode } from "@aether-ui/protocol";
import type { CSSProperties, RefObject } from "react";
import type { GraphCanvasProps } from "./GraphCanvas.tsx";
import { toneToken, type GraphLegendEntry } from "./style.ts";
import type { Surface } from "./surface.ts";
import "./graph.css";

function Legend({ entries }: { entries: readonly GraphLegendEntry[] }) {
  return <ul className="graph-legend" aria-label="Graph legend" data-testid="graph-legend">
    {entries.map((entry) => <li key={entry.label}>
      <span aria-hidden="true" className="graph-swatch" data-shape={entry.shape ?? "rounded-rect"} data-edge={entry.edge}
        style={{ "--graph-tone": `var(${toneToken(entry.tone ?? entry.status ?? "muted")}, var(--color-text-muted))` } as CSSProperties} />
      {entry.label}
    </li>)}
  </ul>;
}

interface OverlayProps {
  props: GraphCanvasProps;
  node: GNode | undefined;
  surface: RefObject<Surface | null>;
  hovered: boolean;
}

export function GraphOverlay({ props, node, surface, hovered }: OverlayProps) {
  if (props.model.nodes.length === 0) return <div className="graph-empty" data-testid="graph-empty">
    {props.emptyState ?? <><strong>No nodes yet</strong><p>Add an item to see its connections here.</p></>}
  </div>;
  return <>
    {props.controls !== false ? <div className="graph-controls" role="group" aria-label="Graph view">
      <button type="button" aria-label="Zoom in" title="Zoom in (+)" data-testid="graph-zoom-in" onClick={() => surface.current?.zoomAt(1.25)}>+</button>
      <button type="button" aria-label="Zoom out" title="Zoom out (−)" data-testid="graph-zoom-out" onClick={() => surface.current?.zoomAt(0.8)}>−</button>
      <button type="button" aria-label="Fit graph" title="Fit graph (0)" data-testid="graph-fit" onClick={() => surface.current?.fit()}>Fit</button>
    </div> : null}
    {props.legend !== undefined && props.legend.length > 0 ? <Legend entries={props.legend} /> : null}
    {node !== undefined && props.details !== undefined ? <div className="graph-details" data-testid="graph-details" aria-label="Node details">
      {props.details(node)}
    </div> : node !== undefined && hovered ? <div className="graph-tooltip" role="tooltip">{node.label}</div> : null}
  </>;
}
