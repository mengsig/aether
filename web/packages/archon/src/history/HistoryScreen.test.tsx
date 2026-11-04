// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { click, CONN, graphHarness, type, type GraphHarness } from "../dag/test/harness.tsx";
import { byText } from "../queue/test/harness.tsx";
import RECORDED from "./fixture/item-history.json";
import HistoryScreen from "./HistoryScreen.tsx";
import { actorLabel } from "./model.ts";

const LABELS: Record<string, string> = { queued: "Queued", dispatching: "Starting", running: "Running", blocked: "Blocked", succeeded: "Succeeded" };

/** Each rendered step as `seq from→to reason by`, read back from the screen. */
function steps(root: HTMLElement): string[] {
  return [...root.querySelectorAll<HTMLElement>(".history-step")].map((step) => {
    const pills = [...step.querySelectorAll(".history-move > *")].map((node) => node.textContent).filter((text) => text !== "→");
    const reason = step.querySelector(".history-reason")?.textContent ?? "-";
    const by = /by (\w+)/.exec(step.querySelector(".history-meta")?.textContent ?? "")?.[1];
    return `${step.dataset.seq} ${pills.join("→")} ${reason} ${by}`;
  });
}

const listed = (root: HTMLElement): string[] => [...root.querySelectorAll(".history-row .dag-row-title")].map((node) => node.textContent ?? "");

async function openHistory(h: GraphHarness, root: HTMLElement, title: string) {
  await h.until(() => root.querySelector(".history-row") !== null, "the search results");
  await click(h, byText(root, ".history-row .dag-row-title", title).closest("button") as HTMLElement);
  await h.until(() => root.querySelector(".history-step") !== null, "the timeline");
}

let h: GraphHarness;
beforeEach(() => { vi.useFakeTimers({ now: Date.parse("2026-11-04T10:00:00Z") }); });
afterEach(() => { h.stop(); vi.restoreAllMocks(); vi.useRealTimers(); });

describe("the history explorer", () => {
  it("reproduces the transitions of a recorded item_history, in order", async () => {
    h = await graphHarness();
    const root = await h.mount(<HistoryScreen />);
    await openHistory(h, root, "Ship dark mode for the docs");
    const expected = RECORDED.data.transitions.map((row) =>
      `${row.seq} ${row.from === null ? "Created" : LABELS[row.from]}→${LABELS[row.to]} ${row.reason ?? "-"} ${actorLabel(row.by)}`);
    expect(steps(root)).toEqual(expected);
    // Each step held until the next: running from 09:20:09 until it blocked at 09:31:40.
    expect(root.querySelector(".history-step[data-seq='15'] .history-meta")?.textContent).toContain("held 11 min");
    expect(root.querySelector(".history-step[data-seq='35'] .history-meta")?.textContent).toContain("21 min so far");
    expect(Object.keys(h.app.store.get().archon[CONN]?.histories ?? {})).toEqual(["qi_dark_mode"]);
  });

  it("searches by text and range with the list_items filters", async () => {
    h = await graphHarness();
    const root = await h.mount(<HistoryScreen />);
    await h.until(() => root.querySelector(".history-row") !== null, "the search results");
    const first = h.faux.inner.sent.find((command) => command.cmd === "list_items" && "updated_since" in command);
    expect(first).toMatchObject({ updated_since: "2026-10-29T00:00:00.000Z", limit: 200 });
    expect(first).not.toHaveProperty("before");
    await type(h, root.querySelector("input[placeholder='Title or brief']") as HTMLInputElement, "sync");
    await type(h, root.querySelectorAll<HTMLInputElement>("input[type='date']")[1] as HTMLInputElement, "2026-11-04");
    await click(h, byText(root, "button", "Search"));
    await h.until(() => listed(root).every((title) => /sync/i.test(title)), "the narrowed results");
    expect(listed(root)).toHaveLength(6);
    expect(h.faux.inner.sent.at(-1)).toMatchObject({ cmd: "list_items", text: "sync", before: "2026-11-05T00:00:00.000Z" });
  });

  it("refuses a range that ends before it starts, and keeps the shown results", async () => {
    h = await graphHarness();
    const root = await h.mount(<HistoryScreen />);
    await h.until(() => root.querySelector(".history-row") !== null, "the search results");
    const sent = h.faux.inner.sent.length;
    await type(h, root.querySelectorAll<HTMLInputElement>("input[type='date']")[1] as HTMLInputElement, "2026-10-01");
    await click(h, byText(root, "button", "Search"));
    expect(root.textContent).toContain("The range starts after it ends.");
    expect(h.faux.inner.sent.length).toBe(sent);
    expect(root.querySelector(".history-row")).not.toBeNull();
  });

  it("appends a live transition while the item's history is shown", async () => {
    h = await graphHarness();
    const root = await h.mount(<HistoryScreen />);
    await openHistory(h, root, "Ship dark mode for the docs");
    h.faux.move("qi_dark_mode", { state: "succeeded", run: "run_dark" });
    await h.until(() => steps(root).length === RECORDED.data.transitions.length + 1, "the new transition");
    expect(steps(root).at(-1)).toMatch(/ Running→Succeeded - Archon$/);
  });
});
