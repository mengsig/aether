import { describe, expect, it, vi } from "vitest";
import { sceneLayout, sceneModel } from "../test/graphs.ts";
import { visibleGraph } from "./model.ts";
import { cubicBezier, interpolate, interpolateCamera, SceneTransition } from "./motion.ts";
import { GraphRenderer, type Context2D, type FrameScheduler, type GraphTheme } from "./renderer.ts";
import { buildScene, hitTest } from "./scene.ts";
import { defaultNodeStyle } from "./style.ts";

const graph = visibleGraph(sceneModel);
const scene = buildScene(graph, sceneLayout, "box", defaultNodeStyle);
const nextLayout = { ...sceneLayout, positions: new Map([...sceneLayout.positions].map(([id, p]) => [id, { x: p.x + 100, y: p.y + 40 }])) };
const target = buildScene(graph, nextLayout, "box", defaultNodeStyle);

describe("graph motion", () => {
  it("interpolates finite values and rejects corrupt progress", () => {
    expect([0, 0.5, 1].map((t) => interpolate(10, 30, t))).toEqual([10, 20, 30]);
    expect(() => interpolate(0, Infinity, 0.5)).toThrow();
    expect(() => interpolate(0, 1, -1)).toThrow();
    expect(() => interpolate(0, 1, NaN)).toThrow();
    const camera = { x: 0, y: 0, scale: 1 };
    interpolateCamera(camera, camera, { x: 10, y: 20, scale: 2 }, 0.5);
    expect(camera).toEqual({ x: 5, y: 10, scale: 1.5 });
  });

  it("uses the token's cubic Bezier and preserves its endpoints", () => {
    const ease = cubicBezier("cubic-bezier(0.2, 0, 0, 1)");
    expect(ease(0)).toBe(0);
    expect(ease(1)).toBe(1);
    expect(ease(0.5)).toBeGreaterThan(0.8);
    expect(ease(0.5)).toBeLessThan(1);
  });

  it("moves nodes, curves and groups without mutating either source scene", () => {
    const transition = new SceneTransition(scene, target);
    transition.step(0.5);
    expect(transition.scene.nodes[0]?.x).toBe((scene.nodes[0]?.x ?? 0) + 50);
    expect(transition.scene.edges[0]?.curve[0].x).toBe((scene.edges[0]?.curve[0].x ?? 0) + 50);
    expect(transition.scene.groups[0]?.y).toBe((scene.groups[0]?.y ?? 0) + 20);
    expect(hitTest(transition.scene, 70, 100)).toEqual({ kind: "node", id: "a" });
    expect(scene.nodes[0]?.x).toBe(16);
    expect(target.nodes[0]?.x).toBe(116);
    transition.step(1);
    expect(transition.scene.nodes.map((n) => n.x)).toEqual(target.nodes.map((n) => n.x));
  });

  it("fades new nodes and preserves a partial fade when interrupted", () => {
    const first = new SceneTransition(undefined, scene);
    first.step(0.25);
    expect(first.scene.nodes[0]?.opacity).toBe(0.25);
    const second = new SceneTransition(first.scene, target);
    second.step(0);
    expect(second.scene.nodes[0]?.opacity).toBe(0.25);
    second.step(1);
    expect(second.scene.nodes[0]?.opacity).toBe(1);
  });

  it("shares one frame loop for layout and camera and stops when idle", () => {
    const frames: (() => void)[] = [];
    const scheduler: FrameScheduler = { request: (f) => frames.push(f), cancel: () => undefined };
    const ctx = { setTransform: () => undefined, clearRect: () => undefined } as unknown as Context2D;
    const renderer = new GraphRenderer(ctx, scheduler, { width: 0, height: 0, dpr: 1, camera: { x: 0, y: 0, scale: 1 }, theme: {} as GraphTheme, scene });
    const now = vi.spyOn(performance, "now").mockReturnValue(0);
    const motion = { duration: 100, ease: (t: number) => t };
    renderer.setScene(target, motion);
    renderer.moveCamera({ x: 10, y: 20, scale: 2 }, motion);
    expect(frames).toHaveLength(1);
    now.mockReturnValue(50);
    frames.shift()?.();
    expect(renderer.scene?.nodes[0]?.x).toBe(66);
    expect(renderer.camera.scale).toBe(1.5);
    expect(frames).toHaveLength(1);
    now.mockReturnValue(100);
    frames.shift()?.();
    expect(renderer.scene).toBe(target);
    expect(renderer.camera.scale).toBe(2);
    expect(frames).toHaveLength(0);
    renderer.setScene(scene, { ...motion, duration: 0 });
    expect(renderer.scene).toBe(scene);
    expect(renderer.animating).toBe(false);
    renderer.dispose();
    now.mockRestore();
  });
});
