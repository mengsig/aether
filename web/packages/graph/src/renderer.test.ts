import { describe, expect, it } from "vitest";
import { sceneLayout, sceneModel } from "../test/graphs.ts";
import { visibleGraph } from "./model.ts";
import { GraphRenderer, type Context2D, type FrameScheduler, type GraphTheme, type RenderState } from "./renderer.ts";
import { buildScene } from "./scene.ts";

/** A 2D context that records the labels and status colours it draws. */
function recordingContext() {
  const drawn = { labels: [] as string[], fills: [] as string[] };
  const ctx = {
    fillStyle: "", strokeStyle: "", lineWidth: 1, font: "", textBaseline: "alphabetic",
    setTransform: () => undefined, clearRect: () => undefined, strokeRect: () => undefined,
    beginPath: () => undefined, moveTo: () => undefined, lineTo: () => undefined, bezierCurveTo: () => undefined, stroke: () => undefined,
    fillRect(this: { fillStyle: string }) { drawn.fills.push(this.fillStyle); },
    fillText: (text: string) => { drawn.labels.push(text); },
    measureText: (text: string) => ({ width: text.length * 7 }),
  };
  return { ctx: ctx as unknown as Context2D, drawn };
}

function manualFrames() {
  const queued: Array<() => void> = [];
  const scheduler: FrameScheduler = { request: (frame) => queued.push(frame), cancel: () => undefined };
  return { scheduler, queued, flush: () => { for (const frame of queued.splice(0)) frame(); } };
}

const theme: GraphTheme = {
  surface: "surface", border: "border", text: "text", muted: "muted", accent: "accent", focus: "focus", edge: "edge",
  font: "12px sans-serif", status: (status) => `status-${status ?? "none"}`,
};

function setup() {
  const { ctx, drawn } = recordingContext();
  const frames = manualFrames();
  const scene = buildScene(visibleGraph(sceneModel), sceneLayout, "box");
  const state: RenderState = { scene, camera: { x: 0, y: 0, scale: 1 }, width: 800, height: 400, dpr: 2, theme };
  return { renderer: new GraphRenderer(ctx, frames.scheduler, state), drawn, frames, scene };
}

describe("GraphRenderer", () => {
  it("coalesces changes into one frame and draws nothing when clean", () => {
    const { renderer, frames } = setup();
    renderer.update({ focused: "a" });
    renderer.update({ hovered: "b" });
    expect(frames.queued).toHaveLength(1);
    frames.flush();
    expect(renderer.framesDrawn).toBe(1);
    renderer.update({ focused: "a", hovered: "b" });
    expect(frames.queued).toHaveLength(0);
    frames.flush();
    expect(renderer.framesDrawn).toBe(1);
  });

  it("redraws a status change from the same scene positions", () => {
    const { renderer, frames, drawn, scene } = setup();
    frames.flush();
    expect(drawn.fills).toContain("status-running");
    const restatused = visibleGraph({ ...sceneModel, nodes: sceneModel.nodes.map((n) => ({ ...n, status: "succeeded" })) });
    renderer.update({ scene: buildScene(restatused, sceneLayout, "box") });
    drawn.fills.length = 0;
    frames.flush();
    expect(renderer.framesDrawn).toBe(2);
    expect(drawn.fills.filter((f) => f.startsWith("status-"))).toEqual(["status-succeeded", "status-succeeded", "status-succeeded"]);
    expect(scene.nodes.map((n) => n.x)).toEqual(buildScene(restatused, sceneLayout, "box").nodes.map((n) => n.x));
  });

  it("culls nodes outside the viewport and hides labels when zoomed far out", () => {
    const { renderer, frames, drawn } = setup();
    renderer.update({ width: 200, height: 400 });
    frames.flush();
    expect(drawn.labels.filter((l) => ["Alpha", "Beta", "Gamma"].includes(l))).toEqual(["Alpha", "Beta"]);
    drawn.labels.length = 0;
    renderer.update({ width: 800, camera: { x: 0, y: 0, scale: 0.2 } });
    frames.flush();
    expect(drawn.labels).toEqual([]);
  });

  it("cuts long labels to the node width with an ellipsis and stops after dispose", () => {
    const { renderer, frames, drawn } = setup();
    const long = visibleGraph({ ...sceneModel, nodes: [{ id: "a", label: "A very long label that cannot fit in a node" }], edges: [] });
    renderer.update({ scene: buildScene(long, sceneLayout, "box") });
    frames.flush();
    const label = drawn.labels.find((l) => l.startsWith("A very"));
    expect(label?.endsWith("…")).toBe(true);
    expect((label?.length ?? 0) * 7).toBeLessThanOrEqual(168 - 20);
    renderer.update({ focused: "a" });
    renderer.dispose();
    frames.flush();
    expect(renderer.framesDrawn).toBe(1);
  });
});
