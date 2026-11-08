// @vitest-environment happy-dom
import type { GraphModel, Layout } from "@aether-ui/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { click, CONN, graphHarness, stubCanvas, type GraphHarness } from "../dag/test/harness.tsx";
import InitiativeScreen from "./InitiativeScreen.tsx";

const gridLayout = async (model: GraphModel): Promise<Layout> =>
  ({ hash: "grid", positions: new Map(model.nodes.map((node, i) => [node.id, { x: i * 220, y: 0 }])) });

const member = (root: HTMLElement, id: string) => root.querySelector<HTMLElement>(`.initiative-member[data-item='${id}']`);
const reads = (h: GraphHarness) => h.faux.inner.sent.filter((command) => command.cmd === "get_initiative").length;

let h: GraphHarness;
beforeEach(() => { vi.useFakeTimers({ now: Date.parse("2026-11-04T10:00:00Z") }); stubCanvas(); });
afterEach(() => { h.stop(); vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });

describe("an initiative page", () => {
  it("shows the contract, each member's live state and why it is held", async () => {
    h = await graphHarness();
    const root = await h.mount(<InitiativeScreen params={{ id: "in_sync" }} layout={gridLayout} />);
    await h.until(() => root.querySelector(".initiative-title") !== null, "the initiative");
    expect(root.querySelector(".initiative-title")?.textContent).toBe("Offline sync");
    expect(root.querySelector(".initiative-contract")?.textContent).toContain("No change is applied twice.");
    expect(root.querySelectorAll(".initiative-member")).toHaveLength(6);
    expect(root.querySelector(".initiative-facts")?.textContent).toContain("Done1 of 6");
    expect(member(root, "qi_sync_api")?.textContent).toContain("Waits on “Store sync cursors” (running)");
    expect(root.querySelector("ul[aria-label^='Initiative graph']")?.textContent).not.toContain("Pairing");
  });

  it("follows a member's move from the change stream and rereads the initiative once", async () => {
    h = await graphHarness();
    const root = await h.mount(<InitiativeScreen params={{ id: "in_sync" }} layout={gridLayout} />);
    await h.until(() => root.querySelector(".initiative-title") !== null, "the initiative");
    const before = reads(h);
    h.faux.move("qi_sync_store", { state: "succeeded", run: "run_s2" });
    await h.until(() => member(root, "qi_sync_store")?.textContent?.includes("Succeeded") === true, "the member's new state");
    await h.until(() => root.querySelector(".initiative-facts")?.textContent?.includes("Done2 of 6") === true, "the new count");
    await h.tick(2_000);
    expect(reads(h)).toBe(before + 1);
    expect(h.app.store.get().archon[CONN]?.initiatives.in_sync).toBeDefined();
  });

  it("shows a member's current title after a rename that keeps its state", async () => {
    h = await graphHarness();
    const root = await h.mount(<InitiativeScreen params={{ id: "in_sync" }} layout={gridLayout} />);
    await h.until(() => root.querySelector(".initiative-title") !== null, "the initiative");
    const entry = h.faux.world.base.items.get("qi_sync_store");
    if (entry === undefined) throw new Error("the demo world has qi_sync_store");
    entry.summary = { ...entry.summary, title: "Renamed sync store" };
    entry.detail = { ...entry.detail, item: { ...(entry.detail.item as object), title: "Renamed sync store" } };
    h.faux.inner.tell("qi_sync_store", { subject: "item", item: "qi_sync_store", state: "running" });
    await h.until(() => h.app.store.get().archon[CONN]?.items.qi_sync_store?.summary.title === "Renamed sync store", "core's renamed summary");
    await h.tick(2_000);
    expect(member(root, "qi_sync_store")?.querySelector(".dag-row-title")?.textContent).toBe("Renamed sync store");
  });

  it("rereads the initiative when only the initiative itself changes", async () => {
    h = await graphHarness();
    const root = await h.mount(<InitiativeScreen params={{ id: "in_sync" }} layout={gridLayout} />);
    await h.until(() => root.querySelector(".initiative-title") !== null, "the initiative");
    const initiative = h.faux.world.initiatives.find((row) => row.id === "in_sync");
    expect(initiative).toBeDefined();
    Object.assign(initiative ?? {}, { state: "shipping" });
    h.faux.inner.tell(null, { subject: "initiative", initiative: "in_sync", state: "shipping" });
    await h.until(() => root.querySelector(".initiative-header")?.textContent?.includes("Shipping") === true, "the shipping initiative");
    expect(h.app.store.get().archon[CONN]?.graph?.initiatives.find((row) => row.id === "in_sync")?.state).toBe("shipping");
  });

  it("rereads the initiative on its change frame even when the state it carries is the same", async () => {
    h = await graphHarness();
    const root = await h.mount(<InitiativeScreen params={{ id: "in_sync" }} layout={gridLayout} />);
    await h.until(() => root.querySelector(".initiative-title") !== null, "the initiative");
    Object.assign(h.faux.world.initiatives.find((row) => row.id === "in_sync") ?? {}, { title: "Offline sync, v2" });
    h.faux.inner.tell(null, { subject: "initiative", initiative: "in_sync", state: "open" });
    await h.until(() => root.querySelector(".initiative-title")?.textContent === "Offline sync, v2", "the renamed initiative");
    expect(reads(h)).toBeGreaterThanOrEqual(2);
  });

  it("says when archond has no such initiative", async () => {
    h = await graphHarness();
    const root = await h.mount(<InitiativeScreen params={{ id: "in_gone" }} layout={gridLayout} />);
    await h.until(() => root.textContent?.includes("No such initiative") === true, "the not-found state");
    expect(root.textContent).toContain("Archon has no initiative in_gone");
    expect(root.querySelector('[role="alert"] button')?.textContent).toBe("Try again");
  });

  it("shows a failed read and retries without losing the connection", async () => {
    h = await graphHarness();
    const read = vi.spyOn(h.app.manager, "readInitiative");
    read.mockRejectedValueOnce(new Error("Request refused"));
    const root = await h.mount(<InitiativeScreen params={{ id: "in_sync" }} layout={gridLayout} />);
    await h.until(() => root.textContent?.includes("Request refused") === true, "the read failure");
    expect(h.app.store.get().connections[CONN]?.state).toBe("ready");
    const retry = [...root.querySelectorAll("button")].find((button) => button.textContent === "Try again");
    if (!retry) throw new Error("the failure offers a retry");
    await click(h, retry);
    await h.until(() => root.querySelector(".initiative-title") !== null, "the retried initiative");
    expect(read).toHaveBeenCalledTimes(2);
  });
});
