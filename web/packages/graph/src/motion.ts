import { invariant } from "./invariant.ts";
import { GridIndex, type Scene, type SceneNode } from "./scene.ts";
import type { Camera, Rect } from "./viewport.ts";

export type Easing = (progress: number) => number;
export interface Motion { duration: number; ease: Easing }
const linear: Easing = (t) => t;

export function interpolate(from: number, to: number, progress: number): number {
  invariant(Number.isFinite(from) && Number.isFinite(to), "interpolation endpoints are finite");
  invariant(Number.isFinite(progress) && progress >= 0 && progress <= 1, "interpolation progress lies in [0, 1]");
  return from + (to - from) * progress;
}

export function cubicBezier(value: string): Easing {
  const parts = /^cubic-bezier\(([^)]+)\)$/.exec(value)?.[1]?.split(",").map(Number);
  if (parts === undefined || parts.length !== 4 || parts.some((n) => !Number.isFinite(n))) return linear;
  const [x1, y1, x2, y2] = parts as [number, number, number, number];
  invariant(x1 >= 0 && x1 <= 1 && x2 >= 0 && x2 <= 1, "Bezier time controls lie in [0, 1]");
  invariant(y1 >= 0 && y1 <= 1 && y2 >= 0 && y2 <= 1, "graph easing does not overshoot");
  const axis = (t: number, a: number, b: number) => 3 * (1 - t) ** 2 * t * a + 3 * (1 - t) * t * t * b + t ** 3;
  return (progress) => {
    if (progress === 0 || progress === 1) return progress;
    let lo = 0, hi = 1;
    for (let i = 0; i < 14; i++) { const mid = (lo + hi) / 2; if (axis(mid, x1, x2) < progress) lo = mid; else hi = mid; }
    return axis((lo + hi) / 2, y1, y2);
  };
}

export function readMotion(element: Element): Motion {
  const style = getComputedStyle(element);
  const raw = style.getPropertyValue("--motion-slow").trim();
  const duration = raw === "" ? 0 : Number.parseFloat(raw) * (raw.endsWith("ms") ? 1 : 1000);
  invariant(Number.isFinite(duration) && duration >= 0, "the motion token is a nonnegative duration");
  invariant(element.isConnected, "motion tokens come from a mounted canvas");
  return { duration, ease: cubicBezier(style.getPropertyValue("--ease-standard").trim()) };
}

function moveRect(out: Rect, from: Rect, to: Rect, t: number): void {
  out.x = interpolate(from.x, to.x, t); out.y = interpolate(from.y, to.y, t);
  out.width = interpolate(from.width, to.width, t); out.height = interpolate(from.height, to.height, t);
}

/** Allocated once per layout; frames mutate only these private drawable copies. */
export class SceneTransition {
  readonly scene: Scene;
  private readonly origins: SceneNode[];
  private readonly groupOrigins: Rect[];
  private readonly target: Scene;

  constructor(previous: Scene | undefined, target: Scene) {
    this.target = target;
    const nodes = target.nodes.map((n) => ({ ...n }));
    this.origins = target.nodes.map((n) => ({ ...(previous?.byId.get(n.node.id) ?? n), opacity: previous?.byId.get(n.node.id)?.opacity ?? (previous?.byId.has(n.node.id) ? 1 : 0) }));
    const byId = new Map(nodes.map((n) => [n.node.id, n]));
    const edges = target.edges.map((e) => ({ ...e, curve: [{ ...e.curve[0] }, { ...e.curve[1] }, { ...e.curve[2] }, { ...e.curve[3] }] as typeof e.curve, box: { ...e.box } }));
    this.scene = { ...target, nodes, byId, edges, index: new GridIndex(nodes, 336, true), groups: target.groups.map((g) => ({ ...g })) };
    const groups = new Map(previous?.groups.map((g) => [g.id, g]));
    this.groupOrigins = target.groups.map((g) => groups.get(g.id) ?? g);
    invariant(this.origins.length === nodes.length, "every animated node has an origin");
    invariant(byId.size === target.byId.size, "animation preserves node identities");
  }

  step(t: number): void {
    for (let i = 0; i < this.scene.nodes.length; i++) {
      const node = this.scene.nodes[i] as SceneNode, start = this.origins[i] as SceneNode;
      moveRect(node, start, this.target.nodes[i] as SceneNode, t);
      node.opacity = interpolate(start.opacity ?? 1, 1, t);
    }
    for (let i = 0; i < this.scene.edges.length; i++) {
      const edge = this.scene.edges[i], target = this.target.edges[i];
      invariant(edge !== undefined && target !== undefined, "animation preserves edge order");
      const a = this.scene.byId.get(target.from.node.id), b = this.scene.byId.get(target.to.node.id);
      invariant(a !== undefined && b !== undefined, "animated edges join placed nodes");
      for (let j = 0; j < 4; j++) {
        const point = edge.curve[j], end = target.curve[j], node = j < 2 ? a : b, dest = j < 2 ? target.from : target.to;
        invariant(point !== undefined && end !== undefined, "a curve has four points");
        point.x = end.x + node.x - dest.x; point.y = end.y + node.y - dest.y;
      }
    }
    for (let i = 0; i < this.scene.groups.length; i++) {
      const group = this.scene.groups[i], target = this.target.groups[i];
      invariant(group !== undefined && target !== undefined, "animation preserves group order");
      moveRect(group, this.groupOrigins[i] ?? target, target, t);
    }
  }
}

export function interpolateCamera(out: { x: number; y: number; scale: number }, from: Camera, to: Camera, t: number): void {
  out.x = interpolate(from.x, to.x, t); out.y = interpolate(from.y, to.y, t); out.scale = interpolate(from.scale, to.scale, t);
}
