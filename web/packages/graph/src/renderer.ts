// Canvas 2D renderer with a dirty flag: every change marks the frame dirty and asks for at most one
// animation frame; a frame with nothing dirty draws nothing. Only what the viewport shows is drawn.

import { interpolateCamera, SceneTransition, type Motion } from "./motion.ts";
import { paintEdge, paintLabel, paintShape } from "./paint.ts";
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
  | "fillStyle" | "strokeStyle" | "lineWidth" | "font" | "textBaseline">
  & Partial<Pick<CanvasRenderingContext2D, "fill" | "strokeText" | "setLineDash" | "globalAlpha" | "textAlign">>;

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
  private transition: { value: SceneTransition; target: Scene; start: number; motion: Motion } | undefined;
  private cameraTransition: { from: Camera; to: Camera; start: number; motion: Motion; current: { x: number; y: number; scale: number } } | undefined;
  private neighbours: ReadonlySet<string> | undefined;
  private styled = false;
  /** Frames that drew; tests read it to prove clean frames cost nothing. */
  framesDrawn = 0;

  private readonly ctx: Context2D;
  private readonly scheduler: FrameScheduler;

  constructor(ctx: Context2D, scheduler: FrameScheduler, initial: RenderState) {
    this.ctx = ctx;
    this.scheduler = scheduler;
    this.state = initial;
    this.styled = initial.scene?.nodes.some((n) => n.style !== undefined) ?? false;
    this.findNeighbours();
    this.invalidate();
  }

  /** Merges changed fields; an update that changes nothing schedules no frame. */
  update(patch: Partial<RenderState>): void {
    const changed = (Object.keys(patch) as Array<keyof RenderState>).some((key) => patch[key] !== this.state[key]);
    if (!changed) return;
    if (patch.theme !== undefined && patch.theme !== this.state.theme) this.labels.clear();
    this.state = { ...this.state, ...patch };
    if ("scene" in patch) this.styled = patch.scene?.nodes.some((n) => n.style !== undefined) ?? false;
    if ("scene" in patch || "hovered" in patch || "selected" in patch || "focused" in patch) this.findNeighbours();
    this.invalidate();
  }

  get scene(): Scene | undefined { return this.state.scene; }
  get camera(): Camera { return this.state.camera; }
  get animating(): boolean { return this.transition !== undefined; }

  setScene(scene: Scene, motion: Motion): void {
    invariant(Number.isFinite(motion.duration) && motion.duration >= 0, "layout motion has a finite nonnegative duration");
    invariant(scene.byId.size === scene.nodes.length, "each drawable node has a unique id");
    this.transition = undefined;
    if (motion.duration > 0 && positionsChanged(this.state.scene, scene)) {
      const value = new SceneTransition(this.state.scene, scene);
      value.step(0);
      this.transition = { value, target: scene, start: performance.now(), motion };
      this.update({ scene: value.scene });
    } else this.update({ scene });
  }

  moveCamera(camera: Camera, motion: Motion): void {
    invariant(Number.isFinite(motion.duration) && motion.duration >= 0, "camera motion has a finite nonnegative duration");
    invariant(Number.isFinite(camera.scale) && camera.scale > 0, "an animated camera has positive scale");
    const current = this.state.camera;
    if (camera.x === current.x && camera.y === current.y && camera.scale === current.scale) {
      this.cameraTransition = undefined;
      return;
    }
    this.cameraTransition = motion.duration === 0 ? undefined
      : { from: this.state.camera, to: camera, current: { ...this.state.camera }, start: performance.now(), motion };
    if (this.cameraTransition === undefined) this.update({ camera });
    else this.invalidate();
  }

  private findNeighbours(): void {
    const id = this.state.hovered ?? this.state.selected ?? this.state.focused;
    if (id === undefined || !this.state.scene?.byId.has(id)) { this.neighbours = undefined; return; }
    const neighbours = new Set([id]);
    for (const edge of this.state.scene?.edges ?? []) {
      if (edge.from.node.id === id) neighbours.add(edge.to.node.id);
      if (edge.to.node.id === id) neighbours.add(edge.from.node.id);
    }
    this.neighbours = neighbours;
  }

  private advance(): void {
    const now = performance.now(), scene = this.transition, camera = this.cameraTransition;
    if (scene !== undefined) {
      const t = Math.min(1, (now - scene.start) / scene.motion.duration);
      scene.value.step(scene.motion.ease(t));
      if (t === 1) { this.state.scene = scene.target; this.transition = undefined; }
    }
    if (camera !== undefined) {
      const t = Math.min(1, (now - camera.start) / camera.motion.duration);
      interpolateCamera(camera.current, camera.from, camera.to, camera.motion.ease(t));
      this.state.camera = camera.current;
      if (t === 1) this.cameraTransition = undefined;
    }
  }

  invalidate(): void {
    this.dirty = true;
    this.handle ??= this.scheduler.request(this.frame);
  }

  dispose(): void {
    if (this.handle !== undefined) this.scheduler.cancel(this.handle);
    this.handle = undefined;
    this.dirty = false;
    this.transition = undefined;
    this.cameraTransition = undefined;
  }

  private readonly frame = (): void => {
    this.handle = undefined;
    if (!this.dirty) return;
    this.dirty = false;
    this.advance();
    this.draw();
    this.framesDrawn++;
    if (this.transition !== undefined || this.cameraTransition !== undefined) this.invalidate();
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
    const { plain, strong } = this.splitEdges(this.animating ? scene.edges : scene.edges.filter((e) => overlaps(e.box, view)));
    this.drawEdges(plain, scene.shape === "box", false);
    if (strong.length > 0) this.drawEdges(strong, scene.shape === "box", true);
    const visible = this.animating ? scene.nodes : scene.index.query(view);
    for (const node of visible) if (overlaps(node, view)) this.drawNode(node, scene.shape === "box", camera.scale);
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
    if (this.styled) {
      for (const edge of edges) {
        ctx.globalAlpha = this.neighbours === undefined || (this.neighbours.has(edge.from.node.id) && this.neighbours.has(edge.to.node.id)) ? 1 : 0.2;
        paintEdge(ctx, edge, this.state.camera.scale, strong);
      }
      ctx.globalAlpha = 1;
      return;
    }
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
    if (scene.style !== undefined) { this.drawStyledNode(scene, scale); return; }
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

  private drawStyledNode(scene: SceneNode, scale: number): void {
    const { ctx, state } = this, { node, style } = scene;
    const related = this.neighbours === undefined || this.neighbours.has(node.id);
    ctx.globalAlpha = (scene.opacity ?? 1) * (related ? 1 : 0.25);
    ctx.fillStyle = state.theme.surface;
    const active = state.hovered === node.id || state.selected === node.id || state.focused === node.id;
    ctx.strokeStyle = state.focused === node.id ? state.theme.focus : active || state.emphasis?.nodes.has(node.id)
      ? state.theme.accent : state.theme.status(style?.tone ?? node.status);
    ctx.lineWidth = (active ? 2.5 : 1.5) / scale;
    paintShape(ctx, scene);
    if (scale >= LABEL_SCALE) {
      const inset = style?.shape === "diamond" ? scene.width * 0.25 : 12;
      const icon = style?.icon === undefined ? "" : `${style.icon} `;
      const label = `${icon}${node.label}${node.collapsedMembers === undefined ? "" : ` (${node.collapsedMembers})`}`;
      const fitted = this.fit(label, Math.max(1, scene.width - 2 * inset));
      ctx.globalAlpha = scene.opacity ?? 1;
      ctx.textAlign = "center";
      paintLabel(ctx, state.theme, fitted, scene.x + scene.width / 2, scene.y + scene.height / 2, related ? state.theme.text : state.theme.muted);
      ctx.textAlign = "left";
    }
    ctx.globalAlpha = 1;
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

function positionsChanged(previous: Scene | undefined, next: Scene): boolean {
  if (previous === undefined || previous.nodes.length !== next.nodes.length) return true;
  return next.nodes.some((node) => {
    const before = previous.byId.get(node.node.id);
    return before === undefined || before.x !== node.x || before.y !== node.y || before.width !== node.width || before.height !== node.height;
  });
}
