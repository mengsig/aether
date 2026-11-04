// The React face of the graph: a focusable canvas drawn by GraphRenderer, pointer pan and zoom,
// keyboard focus and the screen-reader list. Data arrives as props; the layout is never computed here.

import type { GraphModel, Layout } from "@aether-ui/protocol";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode, type RefObject } from "react";
import { announce, keyAction, readingOrder, type KeyAction } from "./focus.ts";
import { invariant } from "./invariant.ts";
import { visibleGraph } from "./model.ts";
import { GraphNodeList, VISUALLY_HIDDEN } from "./NodeList.tsx";
import { animationFrames, GraphRenderer } from "./renderer.ts";
import { buildScene, hitTest, type Hit, type NodeShape, type Scene } from "./scene.ts";
import { readTheme, watchTheme } from "./theme.ts";
import { toWorld, Viewport } from "./viewport.ts";

export interface GraphCanvasProps {
  model: GraphModel;
  /** From layeredLayout or forceLayout; nodes it does not place are listed but not drawn. */
  layout: Layout | undefined;
  /** The accessible name of the graph. */
  label: string;
  shape?: NodeShape;
  selected?: string;
  onActivate?: (id: string) => void;
  onToggleGroup?: (group: string) => void;
}

/** The renderer and viewport behind one mounted canvas. */
class Surface {
  readonly viewport = new Viewport();
  readonly renderer: GraphRenderer;
  width = 0;
  height = 0;
  private scene: Scene | undefined;

  constructor(canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext("2d");
    if (ctx === null) throw new Error("GraphCanvas needs a 2D canvas context");
    this.renderer = new GraphRenderer(ctx, animationFrames,
      { camera: this.viewport.camera, width: 0, height: 0, dpr: devicePixelRatio, theme: readTheme(canvas) });
  }

  /** Rereads the tokens after a theme switch; the layout and the view stay as they are. */
  retheme(canvas: HTMLCanvasElement): void {
    this.renderer.update({ theme: readTheme(canvas) });
  }

  setScene(scene: Scene): void {
    this.scene = scene;
    this.renderer.update({ scene });
    if (!this.viewport.moved) this.fit();
  }

  resize(canvas: HTMLCanvasElement): void {
    const dpr = devicePixelRatio;
    this.width = canvas.clientWidth;
    this.height = canvas.clientHeight;
    canvas.width = Math.round(this.width * dpr);
    canvas.height = Math.round(this.height * dpr);
    this.renderer.update({ width: this.width, height: this.height, dpr });
    // Resizing the backing store clears it even when the CSS size is unchanged.
    this.renderer.invalidate();
    if (!this.viewport.moved) this.fit();
  }

  fit(): void {
    if (this.scene === undefined || this.width === 0 || this.height === 0) return;
    this.renderer.update({ camera: this.viewport.fit(this.scene.bounds, this.width, this.height) });
  }

  panBy(dx: number, dy: number): void {
    this.renderer.update({ camera: this.viewport.panBy(dx, dy) });
  }

  zoomAt(factor: number, x = this.width / 2, y = this.height / 2): void {
    this.renderer.update({ camera: this.viewport.zoomAt(factor, x, y) });
  }

  /** Pans so the focused node or group is on screen. */
  reveal(id: string): void {
    const target = this.scene?.byId.get(id) ?? this.scene?.groups.find((g) => g.id === id);
    if (target === undefined || this.width === 0 || this.height === 0) return;
    this.renderer.update({ camera: this.viewport.reveal(target, this.width, this.height) });
  }

  hitAt(screenX: number, screenY: number): Hit | undefined {
    if (this.scene === undefined) return undefined;
    const world = toWorld(this.viewport.camera, screenX, screenY);
    return hitTest(this.scene, world.x, world.y);
  }
}

/** Creates the surface for the canvas and keeps its backing store sized to the element. */
function useSurface(canvasRef: RefObject<HTMLCanvasElement | null>): RefObject<Surface | null> {
  const surface = useRef<Surface | null>(null);
  useEffect(() => {
    const canvas = canvasRef.current;
    invariant(canvas !== null, "GraphCanvas mounts its canvas before its effects run");
    const current = new Surface(canvas);
    const observer = new ResizeObserver(() => current.resize(canvas));
    observer.observe(canvas);
    const unwatch = watchTheme(canvas, () => current.retheme(canvas));
    surface.current = current;
    return () => { observer.disconnect(); unwatch(); current.renderer.dispose(); surface.current = null; };
  }, [canvasRef]);
  return surface;
}

function local(canvas: HTMLCanvasElement, event: { clientX: number; clientY: number }): [number, number] {
  const box = canvas.getBoundingClientRect();
  return [event.clientX - box.left, event.clientY - box.top];
}

/** Drag pans, the wheel zooms at the pointer, and a click without a drag hits. */
function usePointer(canvasRef: RefObject<HTMLCanvasElement | null>, surface: RefObject<Surface | null>, onHit: (hit: Hit | undefined) => void): void {
  const onHitRef = useRef(onHit);
  useEffect(() => { onHitRef.current = onHit; });
  useEffect(() => {
    const canvas = canvasRef.current;
    invariant(canvas !== null, "GraphCanvas mounts its canvas before its effects run");
    let drag: { x: number; y: number; moved: boolean } | undefined;
    const down = (e: PointerEvent) => {
      if (e.button !== 0) return;
      drag = { x: e.clientX, y: e.clientY, moved: false };
      canvas.setPointerCapture(e.pointerId);
    };
    const move = (e: PointerEvent) => {
      const s = surface.current;
      if (s === null) return;
      if (drag === undefined) return s.renderer.update({ hovered: s.hitAt(...local(canvas, e))?.id });
      if (!drag.moved && Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 3) return;
      drag.moved = true;
      s.panBy(e.clientX - drag.x, e.clientY - drag.y);
      drag.x = e.clientX;
      drag.y = e.clientY;
    };
    const up = (e: PointerEvent) => {
      const s = surface.current;
      const click = drag !== undefined && !drag.moved;
      drag = undefined;
      if (click && s !== null) onHitRef.current(s.hitAt(...local(canvas, e)));
    };
    const cancel = () => { drag = undefined; };
    const wheel = (e: WheelEvent) => {
      const s = surface.current;
      if (s === null) return;
      e.preventDefault();
      s.zoomAt(Math.exp(-e.deltaY * 0.002), ...local(canvas, e));
    };
    const listeners = [["pointerdown", down], ["pointermove", move], ["pointerup", up], ["pointercancel", cancel]] as const;
    for (const [type, listener] of listeners) canvas.addEventListener(type, listener);
    canvas.addEventListener("wheel", wheel, { passive: false });
    return () => {
      for (const [type, listener] of listeners) canvas.removeEventListener(type, listener);
      canvas.removeEventListener("wheel", wheel);
    };
  }, [canvasRef, surface]);
}

export function GraphCanvas(props: GraphCanvasProps): ReactNode {
  const { model, layout, label, selected, onActivate, onToggleGroup } = props;
  const shape = props.shape ?? "box";
  const graph = useMemo(() => visibleGraph(model), [model]);
  const scene = useMemo(() => buildScene(graph, layout, shape), [graph, layout, shape]);
  const listed = useMemo(() => listOrder(graph.nodes, scene), [graph, scene]);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const surface = useSurface(canvasRef);
  const [focused, setFocused] = useState<string>();

  useEffect(() => { surface.current?.setScene(scene); }, [surface, scene]);
  useEffect(() => { surface.current?.renderer.update({ selected }); }, [surface, selected]);
  // Only a focus move pans to the node; a new scene (a status change) keeps the user's view.
  useEffect(() => {
    surface.current?.renderer.update({ focused });
    if (focused !== undefined) surface.current?.reveal(focused);
  }, [surface, focused]);

  usePointer(canvasRef, surface, (hit) => {
    if (hit === undefined) return;
    setFocused(hit.id);
    if (hit.kind === "group" || scene.byId.get(hit.id)?.node.collapsedMembers !== undefined) onToggleGroup?.(hit.id);
    else onActivate?.(hit.id);
  });
  const onKeyDown = (event: KeyboardEvent<HTMLCanvasElement>) => {
    const action = keyAction(scene, focused, event.key);
    if (action === undefined || event.altKey || event.ctrlKey || event.metaKey) return;
    event.preventDefault();
    perform(action, { surface: surface.current, setFocused, onActivate, onToggleGroup });
  };

  return (
    <div style={{ position: "relative", width: "100%", height: "100%" }}>
      <canvas ref={canvasRef} tabIndex={0} role="application" aria-label={label} aria-roledescription="graph"
        style={{ width: "100%", height: "100%", display: "block", touchAction: "none" }} onKeyDown={onKeyDown} />
      <GraphNodeList label={label} nodes={listed} groups={graph.groups} focused={focused}
        onFocusNode={setFocused} onActivate={onActivate} onToggleGroup={onToggleGroup} />
      <div aria-live="polite" style={VISUALLY_HIDDEN}>{announce(scene, focused)}</div>
    </div>
  );
}

/** Placed nodes in reading order, then any the layout has not placed yet. */
function listOrder(nodes: readonly Scene["nodes"][number]["node"][], scene: Scene) {
  const placed = readingOrder(scene.nodes).map((n) => n.node);
  return placed.length === nodes.length ? placed : [...placed, ...nodes.filter((n) => !scene.byId.has(n.id))];
}

interface Handlers {
  surface: Surface | null;
  setFocused: (id: string | undefined) => void;
  onActivate: ((id: string) => void) | undefined;
  onToggleGroup: ((group: string) => void) | undefined;
}

function perform(action: KeyAction, h: Handlers): void {
  const s = h.surface;
  switch (action.kind) {
    case "focus": return h.setFocused(action.id);
    case "blur": return h.setFocused(undefined);
    case "activate": return h.onActivate?.(action.id);
    case "toggle": return h.onToggleGroup?.(action.group);
    case "zoom": return s?.zoomAt(action.factor);
    case "fit": return s?.fit();
  }
}
