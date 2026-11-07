// The 60 fps budget for a 2,000-item DAG, measured on the renderer the screen uses: the real layered
// layout (run inline), the real scene, and the time of each pan and zoom frame. The context is a
// stub, so this is the JS cost per frame; dev/archon-dag-bench.ts measures the same steps in Chromium.

import { buildScene, GraphRenderer, inlineLayoutWorker, LayoutEngine, Viewport, visibleGraph, type FrameScheduler, type GraphTheme } from "@aether-ui/graph";
import type { ArchonGraph } from "@aether-ui/core";
import { describe, expect, it } from "vitest";
import { FauxGraphArchond } from "./fixture/faux-graph.ts";
import { largeGraphWorld } from "./fixture/graph-world.ts";
import { criticalPath, dagData, dagDisplay, dagStructure } from "./model.ts";

const BUDGET_MS = 1000 / 60;
const WIDTH = 1280;
const HEIGHT = 800;

const ctx = {
  fillStyle: "", strokeStyle: "", lineWidth: 1, font: "", textBaseline: "alphabetic",
  setTransform: () => undefined, clearRect: () => undefined, strokeRect: () => undefined, beginPath: () => undefined,
  moveTo: () => undefined, lineTo: () => undefined, bezierCurveTo: () => undefined, stroke: () => undefined,
  fillRect: () => undefined, fillText: () => undefined, measureText: (text: string) => ({ width: text.length * 7 }),
};
const theme: GraphTheme = {
  surface: "#fff", border: "#ddd", text: "#111", muted: "#666", accent: "#06c", focus: "#06c", edge: "#999",
  font: "12px sans-serif", status: (status) => `#${(status ?? "none").length}0${(status ?? "none").length}`,
};

function largeGraph(count: number): ArchonGraph {
  const faux = new FauxGraphArchond(largeGraphWorld(count), { protocol: 24 });
  const reply = faux.graphReply(2000) as { nodes: { id: string }[]; edges: Record<string, unknown>[]; initiatives: Record<string, unknown>[] };
  return { nodes: Object.fromEntries(reply.nodes.map((node) => [node.id, node])), edges: reply.edges, initiatives: reply.initiatives, asOf: 0, truncated: false };
}

const percentile = (sorted: readonly number[], p: number): number => sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] ?? NaN;

describe("the DAG's frame budget", () => {
  it("pans and zooms 2,000 items within 16.7 ms a frame", async () => {
    const data = dagData(largeGraph(2000));
    expect(data.nodes).toHaveLength(2000);
    const structure = dagStructure(data, new Set());
    const engine = new LayoutEngine({ createWorker: inlineLayoutWorker });
    const layout = await engine.layout(structure.model, "layered");
    engine.dispose();
    const scene = buildScene(visibleGraph(dagDisplay(data, structure.model)), layout, "box");
    expect(scene.unplaced).toBe(0);
    const queued: (() => void)[] = [];
    const scheduler: FrameScheduler = { request: (frame) => queued.push(frame), cancel: () => undefined };
    const viewport = new Viewport();
    const camera = viewport.fit(scene.bounds, WIDTH, HEIGHT);
    const renderer = new GraphRenderer(ctx as never, scheduler, { scene, camera, width: WIDTH, height: HEIGHT, dpr: 2, theme, emphasis: criticalPath(data).emphasis });
    const frame = (next: typeof camera): number => {
      renderer.update({ camera: next });
      const started = performance.now();
      for (const run of queued.splice(0)) run();
      return performance.now() - started;
    };
    frame(camera);
    // Zoom in to readable labels over the middle, pan across, zoom back out: 360 steps a user could make.
    const times: number[] = [];
    for (let step = 0; step < 120; step++) times.push(frame(viewport.zoomAt(1.03, WIDTH / 2, HEIGHT / 2)));
    for (let step = 0; step < 120; step++) times.push(frame(viewport.panBy(step < 60 ? -18 : 18, step % 2 === 0 ? 6 : -6)));
    for (let step = 0; step < 120; step++) times.push(frame(viewport.zoomAt(1 / 1.03, WIDTH / 3, HEIGHT / 3)));
    expect(renderer.framesDrawn).toBe(361);
    const sorted = [...times].sort((a, b) => a - b);
    const [p50, p95, max] = [percentile(sorted, 0.5), percentile(sorted, 0.95), sorted.at(-1) ?? NaN];
    console.log(`DAG frame time, 2000 items / ${data.edges.length} links / ${scene.groups.length} groups, 360 pan+zoom steps: p50 ${p50.toFixed(2)} ms, p95 ${p95.toFixed(2)} ms, max ${max.toFixed(2)} ms (budget ${BUDGET_MS.toFixed(1)} ms)`);
    expect(p95).toBeLessThan(BUDGET_MS);
    expect(p50).toBeLessThan(BUDGET_MS / 2);
  });
});
