import { useEffect, useRef, type RefObject } from "react";
import { invariant } from "./invariant.ts";
import { readMotion, type Motion } from "./motion.ts";
import { animationFrames, GraphRenderer } from "./renderer.ts";
import { hitTest, type Hit, type Scene } from "./scene.ts";
import { readTheme, watchTheme } from "./theme.ts";
import { toWorld, Viewport } from "./viewport.ts";

/** The renderer and viewport behind one mounted canvas. */
export class Surface {
  readonly viewport = new Viewport();
  readonly renderer: GraphRenderer;
  width = 0;
  height = 0;
  private scene: Scene | undefined;
  private motion: Motion;
  private reduced: MediaQueryList | undefined;

  constructor(canvas: HTMLCanvasElement) {
    this.motion = readMotion(canvas);
    this.reduced = matchMedia("(prefers-reduced-motion: reduce)");
    const ctx = canvas.getContext("2d");
    if (ctx === null) throw new Error("GraphCanvas needs a 2D canvas context");
    this.renderer = new GraphRenderer(ctx, animationFrames,
      { camera: this.viewport.camera, width: 0, height: 0, dpr: devicePixelRatio, theme: readTheme(canvas) });
  }

  /** Rereads the tokens after a theme switch; the layout and the view stay as they are. */
  retheme(canvas: HTMLCanvasElement): void {
    this.motion = readMotion(canvas);
    this.renderer.update({ theme: readTheme(canvas) });
  }

  setScene(scene: Scene): void {
    this.scene = scene;
    this.renderer.setScene(scene, this.animation());
    if (!this.viewport.moved) this.fit();
  }

  resize(canvas: HTMLCanvasElement): void {
    const dpr = devicePixelRatio;
    invariant(Number.isFinite(dpr) && dpr > 0, `devicePixelRatio ${dpr} is not a usable scale`);
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
    this.renderer.moveCamera(this.viewport.fit(this.scene.bounds, this.width, this.height, 64), this.animation());
  }

  panBy(dx: number, dy: number): void {
    const motion = this.animation();
    this.renderer.moveCamera(this.viewport.panBy(dx, dy), { ...motion, duration: motion.duration / 3 });
  }

  zoomAt(factor: number, x = this.width / 2, y = this.height / 2): void {
    this.renderer.moveCamera(this.viewport.zoomAt(factor, x, y), this.animation());
  }

  /** Pans so the focused node or group is on screen. */
  reveal(id: string): void {
    const target = this.scene?.byId.get(id) ?? this.scene?.groups.find((g) => g.id === id);
    if (target === undefined || this.width === 0 || this.height === 0) return;
    this.renderer.moveCamera(this.viewport.reveal(target, this.width, this.height), this.animation());
  }

  hitAt(screenX: number, screenY: number): Hit | undefined {
    const scene = this.renderer.scene;
    if (scene === undefined) return undefined;
    const world = toWorld(this.renderer.camera, screenX, screenY);
    return hitTest(scene, world.x, world.y);
  }
  private animation(): Motion {
    return this.reduced?.matches === true ? { ...this.motion, duration: 0 } : this.motion;
  }

  watchMotion(canvas: HTMLCanvasElement): () => void {
    const changed = () => {
      this.motion = readMotion(canvas);
      if (this.reduced?.matches !== true) return;
      if (this.scene !== undefined) this.renderer.setScene(this.scene, this.animation());
      this.renderer.moveCamera(this.viewport.camera, this.animation());
    };
    this.reduced?.addEventListener("change", changed);
    return () => this.reduced?.removeEventListener("change", changed);
  }

}

/** Calls `onChange` when devicePixelRatio changes, as when the window moves to another monitor. */
function watchPixelRatio(onChange: () => void): () => void {
  invariant(typeof matchMedia === "function", "GraphCanvas needs matchMedia to follow the pixel ratio");
  let query: MediaQueryList | undefined;
  // A resolution query matches one ratio only, so it is rearmed at the new ratio after each change.
  const arm = (): void => {
    query?.removeEventListener("change", changed);
    query = matchMedia(`(resolution: ${devicePixelRatio}dppx)`);
    query.addEventListener("change", changed);
  };
  const changed = (): void => { arm(); onChange(); };
  arm();
  return () => query?.removeEventListener("change", changed);
}

/** Creates the surface for the canvas and keeps its backing store sized to the element. */
export function useSurface(canvasRef: RefObject<HTMLCanvasElement | null>): RefObject<Surface | null> {
  const surface = useRef<Surface | null>(null);
  useEffect(() => {
    const canvas = canvasRef.current;
    invariant(canvas !== null, "GraphCanvas mounts its canvas before its effects run");
    const current = new Surface(canvas);
    const observer = new ResizeObserver(() => current.resize(canvas));
    observer.observe(canvas);
    const unwatch = watchTheme(canvas, () => current.retheme(canvas));
    const unwatchMotion = current.watchMotion(canvas);
    const unwatchPixels = watchPixelRatio(() => current.resize(canvas));
    surface.current = current;
    return () => { observer.disconnect(); unwatchMotion(); unwatch(); unwatchPixels(); current.renderer.dispose(); surface.current = null; };
  }, [canvasRef]);
  return surface;
}

