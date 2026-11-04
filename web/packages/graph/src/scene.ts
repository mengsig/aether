// The drawable scene: node and group rectangles and edge curves in world coordinates, and a
// uniform grid index for hit testing and culling.

import type { Layout } from "@aether-ui/protocol";
import { FORCE_SHAPE } from "./force.ts";
import { at, invariant } from "./invariant.ts";
import { GROUP_HEADER, GROUP_PADDING, LAYERED_SHAPE } from "./layered.ts";
import type { VisibleGraph, VisibleNode } from "./model.ts";
import type { Rect } from "./viewport.ts";

export type NodeShape = "box" | "dot";
export interface SceneNode extends Rect { node: VisibleNode }
export interface SceneGroup extends Rect { id: string; label: string; depth: number; parent?: string }
export interface Point { x: number; y: number }
/** `curve` is a cubic Bezier (start, two controls, end); `box` holds its control points and so the curve. */
export interface SceneEdge { from: SceneNode; to: SceneNode; kind: string; curve: readonly [Point, Point, Point, Point]; box: Rect }
export type Hit = { kind: "node"; id: string } | { kind: "group"; id: string };

export interface Scene {
  shape: NodeShape;
  /** In draw order; a later node is drawn over an earlier one and wins a hit. */
  nodes: readonly SceneNode[];
  byId: ReadonlyMap<string, SceneNode>;
  edges: readonly SceneEdge[];
  /** Outermost first, so inner boxes draw over outer ones. Empty for dots: force layouts have no bands. */
  groups: readonly SceneGroup[];
  /** Everything drawn: nodes, group boxes and edge curves. */
  bounds: Rect;
  index: GridIndex;
  /** Visible nodes the layout has no position for, e.g. while a new layout computes. */
  unplaced: number;
}

export class GridIndex {
  private readonly cells = new Map<string, SceneNode[]>();
  private readonly cell: number;
  /** Each node's place in draw order, so culled results stack like hit testing does. */
  private readonly order = new Map<SceneNode, number>();
  /** Occupied cell range, so a query over empty space scans no empty cells. */
  private extent = { x0: Infinity, x1: -Infinity, y0: Infinity, y1: -Infinity };

  constructor(nodes: readonly SceneNode[], cell: number) {
    invariant(cell > 0, "a grid cell has a positive size");
    this.cell = cell;
    nodes.forEach((node, i) => {
      this.order.set(node, i);
      this.each(node, (bucket) => bucket.push(node));
    });
    invariant(this.order.size === nodes.length, "a node is indexed once");
  }

  /** The topmost node containing the world point. */
  at(x: number, y: number): SceneNode | undefined {
    const bucket = this.cells.get(this.cellKey(Math.floor(x / this.cell), Math.floor(y / this.cell))) ?? [];
    return bucket.findLast((node) => contains(node, x, y));
  }

  /**
   * Nodes overlapping `rect`, each once, in draw order. Scans at most as many cells as are occupied:
   * a range larger than that walks the occupied cells instead.
   */
  query(rect: Rect): SceneNode[] {
    const found = new Set<SceneNode>();
    const collect = (bucket: readonly SceneNode[]) => { for (const node of bucket) if (overlaps(node, rect)) found.add(node); };
    const { x0, x1, y0, y1 } = this.range(rect);
    const span = Math.max(0, x1 - x0 + 1) * Math.max(0, y1 - y0 + 1);
    if (span > this.cells.size) {
      for (const bucket of this.cells.values()) collect(bucket);
    } else {
      for (let cx = x0; cx <= x1; cx++) for (let cy = y0; cy <= y1; cy++) collect(this.cells.get(this.cellKey(cx, cy)) ?? []);
    }
    const rank = (node: SceneNode) => this.order.get(node) ?? 0;
    return [...found].sort((a, b) => rank(a) - rank(b));
  }

  /** The cell range of `rect`, clamped to the occupied cells. */
  private range(rect: Rect) {
    const e = this.extent;
    return {
      x0: Math.max(e.x0, Math.floor(rect.x / this.cell)), x1: Math.min(e.x1, Math.floor((rect.x + rect.width) / this.cell)),
      y0: Math.max(e.y0, Math.floor(rect.y / this.cell)), y1: Math.min(e.y1, Math.floor((rect.y + rect.height) / this.cell)),
    };
  }

  private each(rect: Rect, visit: (bucket: SceneNode[]) => void): void {
    const x0 = Math.floor(rect.x / this.cell), x1 = Math.floor((rect.x + rect.width) / this.cell);
    const y0 = Math.floor(rect.y / this.cell), y1 = Math.floor((rect.y + rect.height) / this.cell);
    const e = this.extent;
    this.extent = { x0: Math.min(e.x0, x0), x1: Math.max(e.x1, x1), y0: Math.min(e.y0, y0), y1: Math.max(e.y1, y1) };
    for (let cx = x0; cx <= x1; cx++) {
      for (let cy = y0; cy <= y1; cy++) {
        const key = this.cellKey(cx, cy);
        let bucket = this.cells.get(key);
        if (bucket === undefined) this.cells.set(key, (bucket = []));
        visit(bucket);
      }
    }
  }

  private cellKey(cx: number, cy: number): string {
    return `${cx},${cy}`;
  }
}

export function contains(rect: Rect, x: number, y: number): boolean {
  return x >= rect.x && x <= rect.x + rect.width && y >= rect.y && y <= rect.y + rect.height;
}

export function overlaps(a: Rect, b: Rect): boolean {
  return a.x <= b.x + b.width && b.x <= a.x + a.width && a.y <= b.y + b.height && b.y <= a.y + a.height;
}

function union(rects: readonly Rect[]): Rect | undefined {
  if (rects.length === 0) return undefined;
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const r of rects) { x0 = Math.min(x0, r.x); y0 = Math.min(y0, r.y); x1 = Math.max(x1, r.x + r.width); y1 = Math.max(y1, r.y + r.height); }
  return { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
}

/** Group boxes, innermost computed first so each outer box encloses its inner ones. */
function groupBoxes(graph: VisibleGraph, nodes: readonly SceneNode[]): SceneGroup[] {
  const members = new Map<string, Rect[]>();
  const add = (group: string, rect: Rect) => {
    const list = members.get(group);
    if (list === undefined) members.set(group, [rect]); else list.push(rect);
  };
  for (const node of nodes) if (node.node.group !== undefined) add(node.node.group, node);
  const boxes: SceneGroup[] = [];
  for (const group of [...graph.groups].reverse()) {
    const inner = union(members.get(group.id) ?? []);
    if (inner === undefined) continue;
    const box: SceneGroup = { id: group.id, label: group.label, depth: group.depth, ...(group.parent !== undefined ? { parent: group.parent } : {}), x: inner.x - GROUP_PADDING,
      y: inner.y - GROUP_PADDING - GROUP_HEADER, width: inner.width + 2 * GROUP_PADDING, height: inner.height + 2 * GROUP_PADDING + GROUP_HEADER };
    boxes.push(box);
    if (group.parent !== undefined) add(group.parent, box);
  }
  return boxes.reverse();
}

export function buildScene(graph: VisibleGraph, layout: Layout | undefined, shape: NodeShape): Scene {
  const size = shape === "box" ? LAYERED_SHAPE : FORCE_SHAPE;
  const nodes: SceneNode[] = [];
  for (const node of graph.nodes) {
    const at = layout?.positions.get(node.id);
    if (at === undefined) continue;
    invariant(Number.isFinite(at.x) && Number.isFinite(at.y), `node ${node.id} has a non-finite position`);
    nodes.push({ node, x: at.x - size.width / 2, y: at.y - size.height / 2, width: size.width, height: size.height });
  }
  const byId = new Map(nodes.map((n) => [n.node.id, n]));
  const edges: SceneEdge[] = [];
  for (const edge of graph.edges) {
    const from = byId.get(edge.from);
    const to = byId.get(edge.to);
    if (from === undefined || to === undefined) continue;
    const curve = edgeCurve(from, to, shape === "box");
    edges.push({ from, to, kind: edge.kind, curve, box: pointsBox(curve) });
  }
  const groups = shape === "box" ? groupBoxes(graph, nodes) : [];
  const bounds = union([...nodes, ...groups, ...edges.map((e) => e.box)]) ?? { x: 0, y: 0, width: 0, height: 0 };
  const index = new GridIndex(nodes, Math.max(size.width, size.height) * 2);
  invariant(nodes.length <= graph.nodes.length, "a scene never holds more nodes than its graph");
  return { shape, nodes, byId, edges, groups, bounds, index, unplaced: graph.nodes.length - nodes.length };
}

/**
 * An edge as a cubic Bezier: right side of the source to left side of the target for layered
 * graphs, a straight centre-to-centre line (controls on the ends) otherwise.
 */
function edgeCurve(from: Rect, to: Rect, layered: boolean): [Point, Point, Point, Point] {
  if (!layered) {
    const a = { x: from.x + from.width / 2, y: from.y + from.height / 2 };
    const b = { x: to.x + to.width / 2, y: to.y + to.height / 2 };
    return [a, a, b, b];
  }
  const x0 = from.x + from.width, y0 = from.y + from.height / 2;
  const x1 = to.x, y1 = to.y + to.height / 2;
  const bend = Math.max(24, Math.abs(x1 - x0) / 2);
  return [{ x: x0, y: y0 }, { x: x0 + bend, y: y0 }, { x: x1 - bend, y: y1 }, { x: x1, y: y1 }];
}

/** The control points' box, which contains the curve, padded 2 px for the stroke. */
function pointsBox(points: readonly Point[]): Rect {
  const xs = points.map((p) => p.x), ys = points.map((p) => p.y);
  const x = Math.min(...xs) - 2, y = Math.min(...ys) - 2;
  return { x, y, width: Math.max(...xs) + 2 - x, height: Math.max(...ys) + 2 - y };
}

/** A node wins over the header band of the innermost group under the point. */
export function hitTest(scene: Scene, x: number, y: number): Hit | undefined {
  const node = scene.index.at(x, y);
  if (node !== undefined) return { kind: "node", id: node.node.id };
  for (let i = scene.groups.length - 1; i >= 0; i--) {
    const group = at(scene.groups, i);
    if (contains({ ...group, height: GROUP_HEADER }, x, y)) return { kind: "group", id: group.id };
  }
  return undefined;
}
