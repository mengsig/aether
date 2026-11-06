// The React face of the graph: a focusable canvas drawn by GraphRenderer, pointer pan and zoom,
// keyboard focus and the screen-reader list. Data arrives as props; the layout is never computed here.

import type { GNode, GraphModel, Layout } from "@aether-ui/protocol";
import { useEffect, useMemo, useRef, type ReactNode } from "react";
import { announce, readingOrder } from "./focus.ts";
import { visibleGraph } from "./model.ts";
import { GraphNodeList, VISUALLY_HIDDEN } from "./NodeList.tsx";
import type { Emphasis } from "./renderer.ts";
import { buildScene, type NodeShape, type Scene } from "./scene.ts";
import { useSurface } from "./surface.ts";
import { useInteraction } from "./interaction.ts";
import { defaultNodeStyle, type NodeStyler, type GraphLegendEntry } from "./style.ts";
import { GraphOverlay } from "./GraphOverlay.tsx";

export interface GraphCanvasProps {
  model: GraphModel;
  /** From layeredLayout or forceLayout; nodes it does not place are listed but not drawn. */
  layout: Layout | undefined;
  /** The accessible name of the graph. */
  label: string;
  shape?: NodeShape;
  selected?: string;
  /** Drawn in the accent without a relayout, e.g. a critical path; keep its identity stable between renders. */
  emphasis?: Emphasis;
  onActivate?: (id: string) => void;
  onToggleGroup?: (group: string) => void;
  /** Semantic shapes, token tones and optional glyphs, resolved once per scene. */
  nodeStyle?: NodeStyler;
  /** Defaults to true; keyboard and pointer controls remain available when hidden. */
  controls?: boolean;
  legend?: readonly GraphLegendEntry[];
  onHover?: (node: GNode | null) => void;
  onSelect?: (node: GNode | null) => void;
  /** A side card for the hovered, selected or keyboard-focused node. */
  details?: (node: GNode) => ReactNode;
  emptyState?: ReactNode;
}

export function GraphCanvas(props: GraphCanvasProps): ReactNode {
  const { model, layout, label, selected, onToggleGroup } = props;
  const shape = props.shape ?? "box";
  const graph = useMemo(() => visibleGraph(model), [model]);
  const nodeStyle = props.nodeStyle ?? (props.shape === undefined ? defaultNodeStyle : undefined);
  const scene = useMemo(() => buildScene(graph, layout, shape, nodeStyle), [graph, layout, shape, nodeStyle]);
  const listed = useMemo(() => listOrder(graph.nodes, scene), [graph, scene]);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const surface = useSurface(canvasRef);
  const { focused, hovered, focus, activate, onKeyDown } = useInteraction(props, scene, canvasRef, surface);
  useEffect(() => { surface.current?.setScene(scene); }, [surface, scene]);

  return (
    <div className="graph-surface">
      <canvas ref={canvasRef} tabIndex={model.nodes.length === 0 ? -1 : 0} aria-hidden={model.nodes.length === 0 || undefined} title={scene.byId.get(hovered ?? "")?.node.label} role="application" aria-label={label} aria-roledescription="graph"
        style={{ width: "100%", height: "100%", display: "block", touchAction: "none" }} onKeyDown={onKeyDown} />
      <GraphOverlay props={props} node={scene.byId.get(hovered ?? selected ?? focused ?? "")?.node}
        surface={surface} hovered={hovered !== undefined} />
      <GraphNodeList label={label} nodes={listed} groups={graph.groups} focused={focused}
        onFocusNode={focus} onActivate={activate} onToggleGroup={onToggleGroup} />
      <div aria-live="polite" style={VISUALLY_HIDDEN}>{announce(scene, hovered ?? selected ?? focused)}</div>
    </div>
  );
}

/** Placed nodes in reading order, then any the layout has not placed yet. */
function listOrder(nodes: readonly Scene["nodes"][number]["node"][], scene: Scene) {
  const placed = readingOrder(scene.nodes).map((n) => n.node);
  return placed.length === nodes.length ? placed : [...placed, ...nodes.filter((n) => !scene.byId.has(n.id))];
}
