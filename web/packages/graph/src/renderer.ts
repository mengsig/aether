// Canvas 2D renderer with a dirty flag: every change marks the frame dirty and asks for at most one
// animation frame; a frame with nothing dirty draws nothing. Only what the viewport shows is drawn.

import { invariant } from "./invariant.ts";
import { GROUP_HEADER } from "./layered.ts";
import { overlaps, type Scene, type SceneEdge, type SceneNode } from "./scene.ts";
import { visibleWorld, type Camera } from "./viewport.ts";

/** Colours resolved from the design tokens (CONTRACTS.md §4) by the component. */
export interface GraphTheme {
  surface: string; border: string; text: string; muted: string; accent: string; focus: string; edge: string;
  font: string;
  status: (status: string | undefined) => string;
}
export interface FrameScheduler { request(frame: () => void): number; cancel(handle: number): void }
export type Context2D = Pick<CanvasRenderingContext2D,
  "setTransform" | "clearRect" | "fillRect" | "strokeRect" | "fillText" | "measureText" | "beginPath" | "moveTo"
  | "bezierCurveTo" | "lineTo" | "stroke"
  | "fillStyle" | "strokeStyle" | "lineWidth" | "font" | "textBaseline">;

/** Nodes and edges drawn in the accent, such as a critical path; an edge's key is `edgeKey(from, to)`. */
export interface Emphasis { readonly nodes: ReadonlySet<string>; readonly edges: ReadonlySet<string> }
export const edgeKey = (from: string, to: string): string => `${from}\n${to}`;

export interface RenderState { scene?: Scene | undefined; camera: Camera; width: number; height: number; dpr: number;
  focused?: string | undefined; selected?: string | undefined; hovered?: string | undefined; theme: GraphTheme;
  emphasis?: Emphasis | undefined }

/** Labels are skipped below this zoom, where they would be unreadable. */
const LABEL_SCALE = 0.45;

export class GraphRenderer {
  private state: RenderState;
  private dirty = false;
  private handle: number | undefined;
  private readonly labels = new Map<string, string>();
  /** Frames that drew; tests read it to prove clean frames cost nothing. */
  framesDrawn = 0;

  private readonly ctx: Context2D;
  private readonly scheduler: FrameScheduler;

  constructor(ctx: Context2D, scheduler: FrameScheduler, initial: RenderState) {
    this.ctx = ctx;
    this.scheduler = scheduler;
    this.state = initial;
    this.invalidate();
  }

  /** Merges changed fields; an update that changes nothing schedules no frame. */
  update(patch: Partial<RenderState>): void {
    const changed = (Object.keys(patch) as Array<keyof RenderState>).some((key) => patch[key] !== this.state[key]);
    if (!changed) return;
    if (patch.theme !== undefined && patch.theme !== this.state.theme) this.labels.clear();
    this.state = { ...this.state, ...patch };
    this.invalidate();
  }

  invalidate(): void {
    this.dirty = true;
    this.handle ??= this.scheduler.request(this.frame);
  }

  dispose(): void {
    if (this.handle !== undefined) this.scheduler.cancel(this.handle);
    this.handle = undefined;
    this.dirty = false;
  }

  private readonly frame = (): void => {
    this.handle = undefined;
    if (!this.dirty) return;
    this.dirty = false;
    this.draw();
    this.framesDrawn++;
  };

  private draw(): void {
    const { ctx } = this;
    const { scene, camera, width, height, dpr, theme } = this.state;
    invariant(width >= 0 && height >= 0 && dpr > 0, "a canvas has a nonnegative size and a positive pixel ratio");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);
    if (scene === undefined || width === 0 || height === 0) return;
    ctx.setTransform(dpr * camera.scale, 0, 0, dpr * camera.scale, dpr * camera.x, dpr * camera.y);
    const view = visibleWorld(camera, width, height);
    ctx.font = theme.font;
    ctx.textBaseline = "middle";
    for (const group of scene.groups) if (overlaps(group, view)) this.drawGroup(group, camera.scale);
    const { plain, strong } = this.splitEdges(scene.edges.filter((e) => overlaps(e.box, view)));
    this.drawEdges(plain, scene.shape === "box", false);
    if (strong.length > 0) this.drawEdges(strong, scene.shape === "box", true);
    const visible = scene.index.query(view);
    for (const node of visible) this.drawNode(node, scene.shape === "box", camera.scale);
  }

  private drawGroup(group: Scene["groups"][number], scale: number): void {
    const { ctx, state: { theme } } = this;
    ctx.strokeStyle = this.state.focused === group.id ? theme.focus : theme.border;
    ctx.lineWidth = (this.state.focused === group.id ? 3 : 1) / scale;
    ctx.strokeRect(group.x, group.y, group.width, group.height);
    if (scale < LABEL_SCALE) return;
    ctx.fillStyle = theme.muted;
    ctx.fillText(this.fit(group.label, group.width - 16), group.x + 8, group.y + GROUP_HEADER / 2 + 2);
  }

  /** Emphasized edges draw last, over the others. */
  private splitEdges(edges: readonly SceneEdge[]): { plain: readonly SceneEdge[]; strong: readonly SceneEdge[] } {
    const keys = this.state.emphasis?.edges;
    if (keys === undefined || keys.size === 0) return { plain: edges, strong: [] };
    const plain: SceneEdge[] = [];
    const strong: SceneEdge[] = [];
    for (const edge of edges) (keys.has(edgeKey(edge.from.node.id, edge.to.node.id)) ? strong : plain).push(edge);
    invariant(plain.length + strong.length === edges.length, "every edge is drawn once");
    return { plain, strong };
  }

  private drawEdges(edges: readonly SceneEdge[], layered: boolean, strong: boolean): void {
    const { ctx } = this;
    ctx.strokeStyle = strong ? this.state.theme.accent : this.state.theme.edge;
    ctx.lineWidth = (strong ? 2.5 : 1) / this.state.camera.scale;
    ctx.beginPath();
    for (const edge of edges) {
      const [start, c1, c2, end] = edge.curve;
      ctx.moveTo(start.x, start.y);
      if (layered) ctx.bezierCurveTo(c1.x, c1.y, c2.x, c2.y, end.x, end.y); else ctx.lineTo(end.x, end.y);
    }
    ctx.stroke();
  }

  private drawNode(scene: SceneNode, box: boolean, scale: number): void {
    const { ctx, state } = this;
    const { node } = scene;
    ctx.fillStyle = box ? state.theme.surface : state.theme.status(node.status);
    ctx.fillRect(scene.x, scene.y, scene.width, scene.height);
    if (box) {
      ctx.fillStyle = state.theme.status(node.status);
      ctx.fillRect(scene.x, scene.y, 4, scene.height);
    }
    const ring = state.focused === node.id ? state.theme.focus : state.selected === node.id ? state.theme.accent
      : state.hovered === node.id ? state.theme.text : state.theme.border;
    const emphasized = ring === state.theme.border && state.emphasis?.nodes.has(node.id) === true;
    ctx.strokeStyle = emphasized ? state.theme.accent : ring;
    ctx.lineWidth = (emphasized ? 2 : ring === state.theme.border ? 1 : 3) / scale;
    ctx.strokeRect(scene.x, scene.y, scene.width, scene.height);
    if (!box || scale < LABEL_SCALE) return;
    const text = node.collapsedMembers === undefined ? node.label : `${node.label} (${node.collapsedMembers})`;
    ctx.fillStyle = state.theme.text;
    ctx.fillText(this.fit(text, scene.width - 20), scene.x + 12, scene.y + scene.height / 2);
  }

  /** The label cut with an ellipsis to `room` px, cached per text and room. */
  private fit(text: string, room: number): string {
    const key = `${room}\n${text}`;
    const cached = this.labels.get(key);
    if (cached !== undefined) return cached;
    let fitted = text;
    if (this.ctx.measureText(text).width > room) {
      let lo = 0, hi = text.length;
      while (lo < hi) {
        const mid = (lo + hi + 1) >> 1;
        if (this.ctx.measureText(`${text.slice(0, mid)}…`).width <= room) lo = mid; else hi = mid - 1;
      }
      fitted = `${text.slice(0, lo)}…`;
    }
    if (this.labels.size > 10_000) this.labels.clear();
    this.labels.set(key, fitted);
    return fitted;
  }
}

/** The browser's animation frames. */
export const animationFrames: FrameScheduler = {
  request: (frame) => requestAnimationFrame(frame),
  cancel: (handle) => cancelAnimationFrame(handle),
};
