// The task DAG at 2,000 items: mount through the first layout and paint, selecting an item, and one
// item changing state, which must repaint without a relayout (CONTRACTS.md §5.4).

import { selectItems, selectTaskGraph, useSelector, type ArchonGraph, type Store } from "@aether-ui/core";
import { inlineLayoutWorker, LayoutEngine } from "@aether-ui/graph";
import type { ConnectionInfo, GraphModel, Layout } from "@aether-ui/protocol";
import { act, useMemo } from "react";
import { afterAll, beforeAll, describe, expect, test, vi } from "vitest";
import { DagWorkspace } from "../../packages/archon/src/dag/DagScreen.tsx";
import { stubCanvas } from "../../packages/archon/src/dag/test/harness.tsx";
import { DAG_SIZE, itemId, taskGraph, withState } from "../fixtures/dag.ts";
import { heapUsed, mount, perfApp, Results, timed, type Mounted } from "../measure.tsx";

const SEED = 2_000;
const CONN = "archon:perf";
const INFO: ConnectionInfo = { id: CONN, daemon: "archon", state: "ready", protocol: 24, features: ["task_graph", "changes"], standing: "operator", label: "perf" };
const results = new Results("dag");
afterAll(() => results.write());

// Animation frames run when a scenario says so, not on happy-dom's 16 ms timer, so wall time is work.
const rafs = new Map<number, FrameRequestCallback>();
let nextRaf = 1;
let canvasFrames: () => number = () => 0;
beforeAll(() => {
  canvasFrames = stubCanvas().frames;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { rafs.set(nextRaf, callback); return nextRaf++; });
  vi.stubGlobal("cancelAnimationFrame", (handle: number) => { rafs.delete(handle); });
});

async function paint(): Promise<void> {
  await act(async () => {
    const due = [...rafs.values()];
    rafs.clear();
    for (const callback of due) callback(performance.now());
  });
}

/** The DAG workspace over core's stored graph, as DagScreen renders it once archond is ready. */
function LiveDag({ layout }: { readonly layout: (model: GraphModel) => Promise<Layout> }) {
  const graph = useSelector(useMemo(() => selectTaskGraph(CONN), []));
  const items = useSelector(useMemo(() => selectItems(CONN), []));
  if (graph === null) throw new Error("the perf DAG mounts after its graph is stored");
  return <DagWorkspace graph={graph} items={items} layout={layout} initialShowCompleted />;
}

interface Dag { readonly view: Mounted; readonly layouts: () => number; readonly dispose: () => void }

/** A fresh layout engine per mount, so its structure cache cannot hide the first layout's cost. */
async function mountDag(graph: ArchonGraph): Promise<Dag> {
  const { app, frames } = perfApp();
  seed(app.store, graph);
  const engine = new LayoutEngine({ createWorker: inlineLayoutWorker });
  let layouts = 0;
  const layout = (model: GraphModel) => { layouts += 1; return engine.layout(model, "layered"); };
  const view = await mount(app, frames, <LiveDag layout={layout} />);
  for (let turn = 0; turn < 5 && canvasFrames() === 0; turn += 1) { await view.frame(); await paint(); }
  await paint();
  return { view, layouts: () => layouts, dispose: () => { view.unmount(); engine.dispose(); } };
}

function seed(store: Store, graph: ArchonGraph): void {
  store.update((state) => ({ ...state, connections: { ...state.connections, [CONN]: INFO } }));
  store.updateArchon(CONN, (state) => ({ ...state, graph, changeCursor: graph.asOf }));
}

describe("the DAG at 2,000 items", () => {
  const graph = taskGraph(SEED);

  test("mounts through its first layout and paint", async () => {
    expect(Object.keys(graph.nodes)).toHaveLength(DAG_SIZE);
    expect(graph.edges.length).toBeGreaterThan(DAG_SIZE);
    const mountTime = await timed(async () => { (await mountDag(graph)).dispose(); });
    const before = heapUsed();
    const framesBefore = canvasFrames();
    const dag = await mountDag(graph);
    const retained = heapUsed() - before;
    expect(dag.view.container.querySelector(".dag-muted")?.textContent).toContain("2,000 items");
    results.time("dag.mount.ms", mountTime);
    results.count("dag.mount.layouts", dag.layouts());
    results.count("dag.mount.canvas_frames", canvasFrames() - framesBefore);
    results.count("dag.mount.dom_elements", dag.view.container.querySelectorAll("*").length);
    results.amount("dag.mount.heap_bytes", retained);
    dag.dispose();
  });

  test("selects a held-up item", async () => {
    const dag = await mountDag(graph);
    const held = () => dag.view.container.querySelector<HTMLButtonElement>("button.dag-held");
    const overview = async () => {
      const back = [...dag.view.container.querySelectorAll<HTMLButtonElement>(".dag-inspector button")].find((button) => button.textContent === "← Overview");
      if (back) await dag.view.step(() => back.click());
      await paint();
    };
    const select = async () => {
      const pick = held();
      if (!pick) throw new Error("the overview lists no held-up item");
      await dag.view.step(() => pick.click());
      await paint();
    };
    expect(held()).not.toBeNull();
    await dag.view.mutations();
    const [layouts, frames] = [dag.layouts(), canvasFrames()];
    await select();
    expect(dag.view.container.querySelector(".dag-inspector")).not.toBeNull();
    results.count("dag.select.layouts", dag.layouts() - layouts);
    results.count("dag.select.canvas_frames", canvasFrames() - frames);
    results.count("dag.select.dom_mutations", await dag.view.mutations());
    results.time("dag.select.ms", await timed(select, overview));
    dag.dispose();
  });

  test("repaints one item's new state without a relayout", async () => {
    const dag = await mountDag(graph);
    const store = dag.view.app.store;
    let n = 0;
    const change = async () => {
      n += 1;
      await dag.view.step(() => store.updateArchon(CONN, (state) => {
        const held = state.graph;
        if (held === null) throw new Error("the graph left the store");
        return { ...state, graph: withState(held, itemId(1_000), n % 2 === 0 ? "running" : "succeeded") };
      }));
      await paint();
    };
    await dag.view.mutations();
    const [layouts, frames] = [dag.layouts(), canvasFrames()];
    await change();
    results.count("dag.delta.layouts", dag.layouts() - layouts);
    results.count("dag.delta.canvas_frames", canvasFrames() - frames);
    results.count("dag.delta.dom_mutations", await dag.view.mutations());
    results.time("dag.delta.ms", await timed(change));
    dag.dispose();
  });
});
