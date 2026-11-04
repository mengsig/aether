import { describe, expect, it } from "vitest";
import { MAX_SCALE, MIN_SCALE, toWorld, Viewport, visibleWorld } from "./viewport.ts";

describe("Viewport", () => {
  it("zooms about a screen point, keeping the world point under it, within limits", () => {
    const viewport = new Viewport();
    viewport.panBy(30, -10);
    const before = toWorld(viewport.camera, 200, 120);
    const after = toWorld(viewport.zoomAt(2, 200, 120), 200, 120);
    expect(after.x).toBeCloseTo(before.x, 9);
    expect(after.y).toBeCloseTo(before.y, 9);
    expect(viewport.zoomAt(1e6, 0, 0).scale).toBe(MAX_SCALE);
    expect(viewport.zoomAt(1e-9, 0, 0).scale).toBe(MIN_SCALE);
    expect(() => viewport.zoomAt(0, 0, 0)).toThrow(/positive/);
  });

  it("fits bounds centred with padding, never above 1:1, and clears the moved flag", () => {
    const viewport = new Viewport();
    viewport.panBy(5, 5);
    expect(viewport.moved).toBe(true);
    const camera = viewport.fit({ x: 0, y: 0, width: 1000, height: 200 }, 548, 400, 24);
    expect(camera.scale).toBeCloseTo(0.5, 9);
    expect(visibleWorld(camera, 548, 400).x).toBeCloseTo(-48, 9);
    expect(viewport.moved).toBe(false);
    expect(viewport.fit({ x: 0, y: 0, width: 10, height: 10 }, 500, 500).scale).toBe(1);
  });

  it("fits a graph wider than the minimum scale allows, and zooms out to that scale", () => {
    const viewport = new Viewport();
    const bounds = { x: 0, y: 0, width: 480_000, height: 2_000 };
    const camera = viewport.fit(bounds, 800, 600, 24);
    expect(camera.scale).toBeLessThan(MIN_SCALE);
    const world = visibleWorld(camera, 800, 600);
    expect(world.x).toBeLessThanOrEqual(bounds.x);
    expect(world.x + world.width).toBeGreaterThanOrEqual(bounds.width);
    expect(viewport.zoomAt(1e-9, 0, 0).scale).toBe(camera.scale);
  });

  it("reveals an off-screen rectangle with the least pan and leaves a visible one alone", () => {
    const viewport = new Viewport();
    const inside = viewport.reveal({ x: 100, y: 100, width: 50, height: 50 }, 400, 300);
    expect(inside).toEqual({ x: 0, y: 0, scale: 1 });
    const moved = viewport.reveal({ x: 600, y: 100, width: 50, height: 50 }, 400, 300);
    expect(moved.x).toBe(400 - 24 - 650);
    expect(moved.y).toBe(0);
  });
});
