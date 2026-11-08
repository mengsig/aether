// @vitest-environment happy-dom
// Against a faux protocol 23 archond: it refuses the 24 hello as an old archond does, has no change stream,
// run_plan or item_links, and only feeds notifications. The screens fall back to polling (CONTRACTS §5.3.5).
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { POLL_MS } from "../item/detail.ts";
import ItemRoute from "../item/ItemScreen.tsx";
import PipelineRoute from "../pipeline/PipelineScreen.tsx";
import { archonHarness, byText, click, CONN, type ArchonHarness } from "./test/harness.tsx";
import { QueueScreen } from "./QueueScreen.tsx";

let h: ArchonHarness | null = null;
beforeEach(() => { vi.useFakeTimers({ now: Date.parse("2026-11-04T10:00:00Z") }); });
afterEach(() => {
  h?.stop();
  h = null;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

const NOT_IN_23 = ["subscribe_changes", "run_plan", "item_links"];
const sentAny = (harness: ArchonHarness, cmds: readonly string[]) => harness.faux.sent.filter((command) => cmds.includes(String(command.cmd)));

describe("a protocol 23 archond", () => {
  test("is reached by falling back from 24, without the change stream", async () => {
    h = await archonHarness({ protocol: 23 });
    const hellos = h.faux.commands("hello").map((command) => command.protocol_version);
    expect(hellos).toEqual([24, 23]);
    const info = h.app.store.get().connections[CONN];
    expect(info?.state).toBe("ready");
    expect(info?.features).toEqual([]);
    expect(h.faux.commands("subscribe_feed")).toHaveLength(1);
    expect(sentAny(h, NOT_IN_23)).toEqual([]);
  });

  test("refreshes the queue from summaries after a feed event", async () => {
    h = await archonHarness({ protocol: 23 });
    const container = await h.mount(<QueueScreen />);
    expect(container.querySelector(".queue-total")?.textContent).toBe("4 of 7 items · refreshed after each feed event (protocol 23)");
    const before = h.faux.commands("list_item_summaries").length;
    await click(h, byText(container, ".queue-question button", "Approve"));
    const entry = h.world.items.get("qi_rate_limit");
    if (!entry) throw new Error("the demo world has qi_rate_limit");
    entry.summary = { ...entry.summary, state: { state: "failed", error: "x" }, waiting_reason: null };
    h.faux.tell("qi_rate_limit", {});
    await h.until(() => container.querySelector(".queue-total")?.textContent?.startsWith("3 of 7") === true, "the refetched summaries");
    expect(h.faux.commands("list_item_summaries").length).toBeGreaterThan(before);
    expect(sentAny(h, NOT_IN_23)).toEqual([]);
  });

  test("polls an item's detail on feed mentions and every 15 seconds while it runs", async () => {
    h = await archonHarness({ protocol: 23 });
    const container = await h.mount(<ItemRoute params={{ item: "qi_dark_mode" }} />);
    await h.until(() => container.querySelector(".item-title") !== null, "the polled detail");
    expect(container.querySelector(".item-header .item-muted")?.textContent).toContain("polled (protocol 23)");
    const reads = () => h?.faux.commands("get_item").length ?? 0;
    expect(reads()).toBe(1);
    await h.tick(POLL_MS);
    expect(reads()).toBe(2);
    h.faux.tell("qi_dark_mode", {});
    await h.tick(299);
    expect(reads()).toBe(2);
    await h.until(() => reads() === 3, "the follow-up read after the feed mention", 1_000);
    expect(sentAny(h, NOT_IN_23)).toEqual([]);
    expect(container.querySelector('section[aria-label="Links"]')?.textContent).toContain("does not list GitHub links");
    // A 23 summary names the PR only by pr_url, and no issue: each fact stays its own.
    expect([...container.querySelectorAll(".item-fact")].map((fact) => fact.getAttribute("aria-label")))
      .toEqual(["Execution: Running", "Pull request: #57 · state unknown", "Issue: Not linked"]);
    expect(byText(container, ".item-facts-list a", "Open").getAttribute("href")).toBe("https://github.com/acme/site/pull/57");
  });

  test("stops polling a finished item, and refreshes it after a change it made", async () => {
    h = await archonHarness({ protocol: 23 });
    const container = await h.mount(<ItemRoute params={{ item: "qi_changelog" }} />);
    await h.until(() => container.querySelector(".item-title") !== null, "the polled detail");
    await h.tick(POLL_MS * 3);
    expect(h.faux.commands("get_item")).toHaveLength(1);
    const pending = await h.mount(<ItemRoute params={{ item: "qi_rate_limit" }} />);
    await h.until(() => pending.querySelector(".item-title") !== null, "the second item");
    await click(h, byText(pending, ".item-actions > button", "Cancel"));
    await click(h, byText(document, "dialog[open] button", "Cancel item"));
    await h.until(() => pending.querySelector(".item-callout-title")?.textContent === "Cancelled", "the cancelled detail", 1_000);
  });

  test("lists a run's started stages in order without run_plan", async () => {
    h = await archonHarness({ protocol: 23 });
    const container = await h.mount(<PipelineRoute params={{ item: "qi_dark_mode", run: "run_dark" }} />);
    await h.until(() => container.querySelectorAll(".pipeline-step").length > 0, "the observed steps");
    expect([...container.querySelectorAll(".pipeline-name")].map((node) => node.textContent)).toEqual(["plan", "build", "review"]);
    expect(container.querySelector(".pipeline-step .ui-badge[data-tone=warning]")).toBeNull();
    expect(container.querySelector(".pipeline-note")?.textContent).toBe("Plan unavailable. Showing started stages.");
    expect(sentAny(h, NOT_IN_23)).toEqual([]);
  });

  test("lets the operator act, since every 23 device is an operator", async () => {
    h = await archonHarness({ protocol: 23 });
    const container = await h.mount(<QueueScreen />);
    await h.until(() => container.querySelector(".queue-summary") !== null, "the status header");
    expect(byText<HTMLButtonElement>(container, "button", "New item").disabled).toBe(false);
  });
});
