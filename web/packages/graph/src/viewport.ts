// Pan, zoom and fit for a 2D scene. The camera maps world to screen as screen = world * scale + offset.

import { invariant } from "./invariant.ts";

export interface Camera { readonly x: number; readonly y: number; readonly scale: number }
export interface Rect { x: number; y: number; width: number; height: number }

/** The zoom-out floor, lowered by `fit` when a whole graph needs a smaller scale. */
export const MIN_SCALE = 0.05;
export const MAX_SCALE = 4;

export class Viewport {
  private current: Camera = { x: 0, y: 0, scale: 1 };
  private minScale = MIN_SCALE;
  /** True once the user panned or zoomed; a new scene then keeps their view instead of refitting. */
  moved = false;

  get camera(): Camera {
    return this.current;
  }

  panBy(dx: number, dy: number): Camera {
    invariant(Number.isFinite(dx) && Number.isFinite(dy), "pan distances are finite");
    this.current = { ...this.current, x: this.current.x + dx, y: this.current.y + dy };
    this.moved = true;
    return this.current;
  }

  /** Zooms by `factor` keeping the screen point (sx, sy) over the same world point. */
  zoomAt(factor: number, sx: number, sy: number): Camera {
    invariant(Number.isFinite(factor) && factor > 0, "a zoom factor is positive");
    invariant(Number.isFinite(sx) && Number.isFinite(sy), "a zoom origin is finite");
    const { x, y, scale } = this.current;
    const next = Math.min(MAX_SCALE, Math.max(this.minScale, scale * factor));
    this.current = { x: sx - ((sx - x) * next) / scale, y: sy - ((sy - y) * next) / scale, scale: next };
    this.moved = true;
    return this.current;
  }

  /**
   * Centres `bounds` in a width × height screen with `padding` px around it, never above 1:1.
   * A graph too large for MIN_SCALE still fits whole, and zooming may then go down to that scale.
   */
  fit(bounds: Rect, width: number, height: number, padding = 24): Camera {
    invariant(width > 0 && height > 0, "fit needs a screen with an area");
    invariant(bounds.width >= 0 && bounds.height >= 0, "fit needs nonnegative bounds");
    const room = (size: number, extent: number) => (extent === 0 ? MAX_SCALE : Math.max(1, size - 2 * padding) / extent);
    const scale = Math.min(1, room(width, bounds.width), room(height, bounds.height));
    invariant(scale > 0 && Number.isFinite(scale), "a fitted scale is positive and finite");
    this.minScale = Math.min(MIN_SCALE, scale);
    this.current = {
      scale,
      x: width / 2 - (bounds.x + bounds.width / 2) * scale,
      y: height / 2 - (bounds.y + bounds.height / 2) * scale,
    };
    this.moved = false;
    return this.current;
  }

  /** Pans the least distance that brings `rect` (world) inside the screen with `margin` px. */
  reveal(rect: Rect, width: number, height: number, margin = 24): Camera {
    invariant(width > 0 && height > 0, "reveal needs a screen with an area");
    const { x, y, scale } = this.current;
    const dx = shiftInto(rect.x * scale + x, rect.width * scale, width, margin);
    const dy = shiftInto(rect.y * scale + y, rect.height * scale, height, margin);
    return dx === 0 && dy === 0 ? this.current : this.panBy(dx, dy);
  }
}

/** The screen shift that brings [lo, lo + extent] inside [margin, size - margin]; the start wins. */
function shiftInto(lo: number, extent: number, size: number, margin: number): number {
  if (lo < margin) return margin - lo;
  const hi = lo + extent;
  return hi > size - margin ? Math.max(margin - lo, size - margin - hi) : 0;
}

export function toWorld(camera: Camera, sx: number, sy: number): { x: number; y: number } {
  return { x: (sx - camera.x) / camera.scale, y: (sy - camera.y) / camera.scale };
}

/** The world rectangle a width × height screen shows. */
export function visibleWorld(camera: Camera, width: number, height: number): Rect {
  const origin = toWorld(camera, 0, 0);
  return { x: origin.x, y: origin.y, width: width / camera.scale, height: height / camera.scale };
}
