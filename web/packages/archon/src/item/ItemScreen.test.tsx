// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { archonHarness, byText, click, type ArchonHarness } from "../queue/test/harness.tsx";
import { demoWorld } from "../queue/fixture/world.ts";
import ItemRoute from "./ItemScreen.tsx";

let h: ArchonHarness | null = null;
beforeEach(() => { vi.useFakeTimers({ now: Date.parse("2026-11-04T10:00:00Z") }); });
afterEach(() => {
  h?.stop();
  h = null;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

const facts = (container: HTMLElement) => [...container.querySelectorAll(".item-fact")].map((fact) => fact.getAttribute("aria-label"));
const section = (container: HTMLElement, title: string) => container.querySelector(`section[aria-label="${title}"]`);

async function item(id: string, world = demoWorld()) {
  h = await archonHarness({ protocol: 24 }, world);
  const container = await h.mount(<ItemRoute params={{ item: id }} />);
  await h.until(() => container.querySelector(".item-title") !== null, `the detail of ${id}`);
  await h.until(() => container.querySelectorAll(".item-actions button").length > 1, "the status the actions read");
  return { h, container };
}

describe("an item's states", () => {
  test("shows a succeeded item's open PR and open issue as three separate states", async () => {
    const { container } = await item("qi_changelog");
    await vi.waitFor(() => expect(facts(container)).toEqual(["Execution: Succeeded", "Pull request: #52 · Open", "Issue: #38 · Open"]));
    expect(container.querySelector('.item-fact .ui-pill[data-status="pr-open"]')).not.toBeNull();
    expect(container.querySelector('.item-fact .ui-pill[data-status="pr-merged"]')).toBeNull();
    expect(container.querySelector(".item-callout")?.textContent).toBe("DoneChangelog drafted and delivered.");
  });

  test("reads the issue state from GitHub links, and a draft PR with pending checks", async () => {
    const { container } = await item("qi_dark_mode");
    await vi.waitFor(() => expect(facts(container)).toEqual(["Execution: Running", "Pull request: #57 · Draft", "Issue: #41 · Open"]));
    expect(container.querySelector(".item-fact:nth-child(1)")?.textContent).toContain("stage review");
    expect(container.querySelector(".item-fact:nth-child(2)")?.textContent).toContain("checks pending");
    expect(section(container, "Links")?.textContent).toContain("acme/site#41");
  });

  test("says what an item without a PR or issue is", async () => {
    const { container } = await item("qi_rate_limit");
    expect(facts(container)).toEqual(["Execution: Queued", "Pull request: No pull request", "Issue: Not linked"]);
    expect(container.querySelector(".item-callout")?.textContent).toBe("WaitingWaiting for a free agent slot (4 of 4 busy)");
  });
});

describe("an item's sections", () => {
  test("lists runs linking to their pipeline, the stage timeline and the gates", async () => {
    const { container } = await item("qi_pairing");
    expect(section(container, "Runs")?.querySelector("a")?.getAttribute("href")).toBe("#/archon/items/qi_pairing/runs/run_pair");
    expect(section(container, "Runs")?.querySelector(".ui-pill")?.textContent).toBe("Needs you");
    expect(section(container, "Stages")?.textContent).toContain("Proposes 8-character codes.");
    expect(section(container, "Gates")?.textContent).toContain("Proposal · plan");
    expect(section(container, "Gates")?.textContent).toContain("Waiting on you");
    expect(container.querySelector(".queue-question")?.textContent).toContain("Approve the proposal");
  });

  test("links each dependency and shows the brief and cost", async () => {
    const { container } = await item("qi_landing");
    const details = section(container, "Details");
    expect(details?.querySelector("a")?.getAttribute("href")).toBe("#/archon/items/qi_assets");
    expect(details?.textContent).toContain("Total cost$0.00");
    expect(section(container, "Brief")?.textContent).toContain("Acceptance: the change is tested and documented.");
  });

  test("reads an artifact a page at a time", async () => {
    const world = demoWorld();
    const entry = world.items.get("qi_changelog");
    if (!entry) throw new Error("the demo world has qi_changelog");
    entry.artifacts = [{ name: "log.txt", bytes: 70_000, text: `${"a".repeat(65_536)}${"b".repeat(4_464)}` }];
    const { h, container } = await item("qi_changelog", world);
    await h.until(() => section(container, "Artifacts")?.querySelector("button") !== null, "the artifact list");
    await click(h, byText(container, ".item-row-button", /^log\.txt/));
    await h.until(() => document.querySelector(".item-artifact") !== null, "the first page");
    expect(document.querySelector(".item-artifact")?.textContent).toHaveLength(65_536);
    await click(h, byText(document, "button", /^Load more/));
    await h.until(() => document.querySelector(".item-artifact")?.textContent?.length === 70_000, "the second page");
    expect(h.faux.commands("read_artifact").map((command) => [command.offset, command.limit])).toEqual([[0, 65_536], [65_536, 65_536]]);
    expect(document.querySelector("dialog[open]")?.textContent).toContain("End of file");
  });

  test("links a report only through a safe URL", async () => {
    const { container } = await item("qi_changelog");
    await vi.waitFor(() => expect(section(container, "Reports")?.querySelector("a")?.getAttribute("href")).toBe("https://example.com/reports/rp_1"));
  });
});

describe("an item's actions", () => {
  test("cancels after asking, and the change stream refreshes the page", async () => {
    const { h, container } = await item("qi_dark_mode");
    await click(h, byText(container, ".item-actions > button", "Cancel"));
    expect(document.querySelector("dialog[open]")?.textContent).toContain("Cancel this item?");
    await click(h, byText(document, "dialog[open] button", "Cancel item"));
    expect(h.faux.commands("cancel_item")).toEqual([expect.objectContaining({ item: "qi_dark_mode", reason: null })]);
    await h.until(() => facts(container)[0] === "Execution: Cancelled", "the cancelled state");
    expect(byText(container, ".item-actions > button", "Retry")).toBeTruthy();
    // The live state moves at once; core refetches the shown item's detail a moment later.
    await h.until(() => container.querySelector(".item-callout-title")?.textContent === "Cancelled", "the refetched detail");
    expect(h.faux.commands("get_item")).toHaveLength(2);
  });

  test("retries a failed item", async () => {
    const { h, container } = await item("qi_assets");
    await click(h, byText(container, ".item-actions > button", "Retry"));
    await click(h, byText(document, "dialog[open] button", "Retry"));
    expect(h.faux.commands("retry_item")).toEqual([expect.objectContaining({ item: "qi_assets", model_plan: null })]);
    await h.until(() => facts(container)[0] === "Execution: Queued", "the queued state");
  });

  test("pauses and resumes the item's repository and pipeline", async () => {
    const { h, container } = await item("qi_dark_mode");
    await click(h, byText(container, "button", "Pause repository"));
    expect(h.faux.commands("pause")).toEqual([expect.objectContaining({ scope: "repo", name: "repo_site" })]);
    await h.until(() => container.querySelector('[aria-label="Resume repository site"]') !== null, "the paused repository");
    await click(h, byText(container, "button", "Pause pipeline"));
    expect(h.faux.commands("pause").at(-1)).toMatchObject({ scope: "pipeline", name: "adaptive" });
    await h.until(() => container.querySelector('[aria-label="Resume pipeline adaptive"]') !== null, "the paused pipeline");
    await click(h, byText(container, "button", "Resume repository"));
    expect(h.faux.commands("resume")).toEqual([expect.objectContaining({ scope: "repo", name: "repo_site" })]);
  });

  test("shows a daemon refusal beside the actions", async () => {
    const { h, container } = await item("qi_old_flag");
    await click(h, byText(container, ".item-actions > button", "Retry"));
    h.world.items.delete("qi_old_flag");
    await click(h, byText(document, "dialog[open] button", "Retry"));
    expect(container.querySelector(".item-actions [role=alert]")?.textContent).toBe("no item qi_old_flag");
  });

  test("answers the item's question from its page", async () => {
    const { h, container } = await item("qi_pairing");
    await click(h, byText(container, ".queue-question button", "Revise"));
    expect(h.faux.commands("answer")).toEqual([expect.objectContaining({ question: "q_review_gate", text: "Revise" })]);
  });
});

test("says when archond has no such item", async () => {
  h = await archonHarness({ protocol: 24 });
  const container = await h.mount(<ItemRoute params={{ item: "qi_missing" }} />);
  expect(container.querySelector(".ui-state-title")?.textContent).toBe("Item not found");
});
