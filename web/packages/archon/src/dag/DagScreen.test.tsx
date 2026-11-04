// @vitest-environment happy-dom
import type { GraphModel, Layout } from "@aether-ui/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { byText, click } from "../queue/test/harness.tsx";
import DagScreen from "./DagScreen.tsx";
import { CONN, graphHarness, stubCanvas, type GraphHarness } from "./test/harness.tsx";

/** A layout function that counts its calls and places nodes on a grid in model order. */
function countingLayout() {
  const calls: GraphModel[] = [];
  const layout = async (model: GraphModel): Promise<Layout> => {
    calls.push(model);
    return { hash: `l${calls.length}`, positions: new Map(model.nodes.map((node, i) => [node.id, { x: (i % 6) * 220, y: Math.floor(i / 6) * 80 }])) };
  };
  return { calls, layout };
}

const nodeList = (root: HTMLElement): string[] => [...root.querySelectorAll("ul[aria-label^='Task graph'] button")].map((b) => b.textContent ?? "");
const pathTitles = (root: HTMLElement): string[] => [...root.querySelectorAll("ol.dag-path .dag-row-title")].map((n) => n.textContent ?? "");

let h: GraphHarness;
beforeEach(() => { vi.useFakeTimers({ now: Date.parse("2026-11-04T10:00:00Z") }); });
afterEach(() => { h.stop(); vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });

describe("the task DAG", () => {
  it("groups items by initiative and shows the critical path of open work", async () => {
    stubCanvas();
    h = await graphHarness();
    const { calls, layout } = countingLayout();
    const root = await h.mount(<DagScreen layout={layout} />);
    await h.until(() => calls.length === 1 && pathTitles(root).length > 0, "the first layout");
    expect(nodeList(root)).toEqual(expect.arrayContaining(["Group Offline sync", "Group Faster pairing", "Store sync cursors, running"]));
    const groups = new Map(calls[0]?.nodes.map((node) => [node.id, node.group]));
    expect(groups.get("qi_sync_store")).toBe("initiative:in_sync");
    expect(groups.get("qi_pair_ui")).toBe("initiative:in_pairing");
    expect(groups.get("qi_dark_mode")).toBeUndefined();
    // The longest chain of open items; the succeeded schema item is no longer on it.
    expect(pathTitles(root)).toEqual(["Store sync cursors", "Serve the sync endpoint", "Resume sync in the client", "Document offline sync"]);
  });

  it("lists held-up items with their reasons, and the inspector says why one is blocked", async () => {
    stubCanvas();
    h = await graphHarness();
    const { layout } = countingLayout();
    const root = await h.mount(<DagScreen layout={layout} />);
    await h.until(() => root.querySelector(".dag-held") !== null, "the held-up list");
    const landing = root.querySelector<HTMLElement>(".dag-held[data-item='qi_landing']");
    expect(landing?.textContent).toContain("A dependency failed");
    expect(landing?.textContent).toContain("Waits on “Compress the hero images” (failed)");
    const pairUi = root.querySelector<HTMLElement>(".dag-held[data-item='qi_pair_ui']");
    expect(pairUi?.textContent).toContain("The design review asked for a second pass on the copy");
    await click(h, byText(root, ".dag-held .dag-row-title", "Pairing screen copy").closest("button") as HTMLElement);
    const inspector = root.querySelector<HTMLElement>(".dag-inspector[data-item='qi_pair_ui']");
    expect(inspector?.querySelector("[aria-label='Why it is held']")?.textContent).toContain("The design review asked for a second pass on the copy");
    expect(inspector?.querySelector("a[href='#/archon/initiatives/in_pairing']")).not.toBeNull();
  });

  it("repaints a status change without a relayout, and relayouts only when the structure changes", async () => {
    const canvas = stubCanvas();
    h = await graphHarness();
    const { calls, layout } = countingLayout();
    const root = await h.mount(<DagScreen layout={layout} />);
    await h.until(() => calls.length === 1 && nodeList(root).includes("Store sync cursors, running"), "the first layout");
    const graphBefore = h.app.store.get().archon[CONN]?.graph;
    const framesBefore = canvas.frames();

    h.faux.move("qi_sync_store", { state: "succeeded", run: "run_s2" });
    // The change patches the node in place; its moved prerequisite then makes core reread the whole graph.
    await h.until(() => nodeList(root).includes("Store sync cursors, succeeded"), "the status change on screen");
    await h.until(() => h.app.store.get().archon[CONN]?.graph !== graphBefore && h.faux.inner.commands("task_graph").length === 2, "the graph reread");
    await h.tick(1_000);
    expect(nodeList(root)).toContain("Store sync cursors, succeeded");
    // Two chains of three open items remain; the tie goes to the chain ending at the smaller id.
    expect(pathTitles(root)).toEqual(["Shorten the pairing code", "Pairing screen copy", "Pairing end-to-end tests"]);
    expect(canvas.frames()).toBeGreaterThan(framesBefore);
    expect(calls).toHaveLength(1);

    const metrics = h.faux.world.base.items.get("qi_sync_metrics");
    if (!metrics) throw new Error("the demo world has qi_sync_metrics");
    metrics.summary = { ...metrics.summary, dependencies: ["qi_sync_store", "qi_sync_docs"] };
    h.faux.inner.tell("qi_sync_metrics", { subject: "dependency", item: "qi_sync_metrics", depends_on: "qi_sync_docs", added: true });
    await h.until(() => calls.length === 2, "the relayout of the new edge");
    expect(calls[1]?.edges).toContainEqual({ from: "qi_sync_docs", to: "qi_sync_metrics", kind: "depends_on" });
    await h.tick(1_000);
    expect(calls).toHaveLength(2);
  });

  it("explains an empty graph instead of showing a blank canvas", async () => {
    stubCanvas();
    const world = (await import("./fixture/graph-world.ts")).demoGraphWorld();
    world.base.items.clear();
    h = await graphHarness(world);
    const root = await h.mount(<DagScreen layout={countingLayout().layout} />);
    expect(root.querySelector(".ui-state")?.textContent).toContain("Nothing in the graph");
  });
});
