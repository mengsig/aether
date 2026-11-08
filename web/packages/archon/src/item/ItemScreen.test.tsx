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
  test("names the newest delivered PR once the links are reread", async () => {
    const { h, container } = await item("qi_dark_mode");
    await h.until(() => facts(container).includes("Pull request: #57 · Draft"), "the first PR");
    const entry = h.world.items.get("qi_dark_mode");
    if (!entry) throw new Error("the demo world has qi_dark_mode");
    const ref = { host: "github.com", repo: "acme/site", number: 58, kind: "pull_request" };
    const links = entry.links as { links: unknown[]; objects: unknown[] };
    entry.links = { links: [...links.links, { item: "qi_dark_mode", ref, relation: "delivered", source: "ship", created_at: "2026-11-04T10:00:00Z" }], objects: links.objects };
    h.faux.tell("qi_dark_mode", { subject: "link", item: "qi_dark_mode" });
    await h.until(() => facts(container).includes("Pull request: #58 · state unknown"), "the newer PR", 2_000);
  });

  test("names the stage from the reread detail, not the older summary", async () => {
    const { h, container } = await item("qi_dark_mode");
    expect(container.querySelector(".item-fact:nth-child(1)")?.textContent).toContain("stage review");
    const entry = h.world.items.get("qi_dark_mode");
    if (!entry) throw new Error("the demo world has qi_dark_mode");
    const runs = entry.detail.runs as Record<string, unknown>[];
    entry.detail = { ...entry.detail, runs: runs.map((run) => ({ ...run, state: { state: "running", stage_id: "docs", attempt: 1 } })) };
    h.faux.tell("qi_dark_mode", { subject: "stage", item: "qi_dark_mode", run: "run_dark", stage_id: "docs" });
    await h.until(() => container.querySelector(".item-fact:nth-child(1)")?.textContent?.includes("stage docs") === true, "the reread stage", 2_000);
  });

  test("rereads the links, reports and questions after a change names the item", async () => {
    const { h, container } = await item("qi_rate_limit");
    expect(facts(container)).toContain("Issue: Not linked");
    const entry = h.world.items.get("qi_rate_limit");
    if (!entry) throw new Error("the demo world has qi_rate_limit");
    const ref = { host: "github.com", repo: "acme/api", number: 61, kind: "issue" };
    entry.links = { links: [{ item: "qi_rate_limit", ref, relation: "origin", source: "operator", created_at: "2026-11-04T10:00:00Z" }],
      objects: [{ ref, title: "Rate limits", state: "open", labels: [], updated_at: "2026-11-04T10:00:00Z", fetched_at: "2026-11-04T10:00:00Z" }] };
    h.world.questions.push({ id: "q_limit", number: 5, item: "qi_rate_limit", text: "Which limit per minute?", options: [], asked_at: "2026-11-04T10:00:00Z" });
    h.faux.tell("qi_rate_limit", { subject: "question", item: "qi_rate_limit", question: "q_limit", open: true });
    await h.until(() => facts(container).includes("Issue: #61 · Open"), "the reread links", 2_000);
    expect(section(container, "Links")?.textContent).toContain("acme/api#61");
    await h.until(() => container.textContent?.includes("Which limit per minute?") === true, "the reread questions", 2_000);
  });

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
    expect(section(container, "Links")?.querySelector('a[aria-label="Open the page of acme/site#41"]')?.getAttribute("href")).toBe("#/github/github.com/acme/site/41");
  });

  test("says what an item without a PR or issue is", async () => {
    const { container } = await item("qi_rate_limit");
    expect(facts(container)).toEqual(["Execution: Queued", "Pull request: No pull request", "Issue: Not linked"]);
    expect(container.querySelector(".item-callout")?.textContent).toBe("WaitingWaiting for a free agent slot (4 of 4 busy)");
  });
});

describe("an item's sections", () => {
  test("contains long gate descriptions behind an expandable summary", async () => {
    const world = demoWorld();
    const entry = world.items.get("qi_pairing");
    if (!entry) throw new Error("the demo world has qi_pairing");
    const summary = `/workspace/${"long-path".repeat(40)}`;
    entry.detail = { ...entry.detail, gates: [{ ...(entry.detail.gates as Record<string, unknown>[])[0], summary }] };
    const { container } = await item("qi_pairing", world);
    const details = section(container, "Gates")?.querySelector("details");
    expect(details?.open).toBe(false);
    expect(details?.querySelector("summary")?.textContent?.length).toBeLessThan(summary.length);
    expect(details?.querySelector("p")?.textContent).toBe(summary);
  });

  test("lists runs linking to their pipeline, the stage timeline and the gates", async () => {
    const { container } = await item("qi_pairing");
    expect(section(container, "Runs")?.querySelector("a")?.getAttribute("href")).toBe("#/archon/items/qi_pairing/runs/run_pair");
    expect(section(container, "Runs")?.querySelector(".ui-pill")?.textContent).toBe("Needs you");
    expect(section(container, "Stages")?.textContent).toContain("Proposes 8-character codes.");
    expect(section(container, "Gates")?.textContent).toContain("Proposal · plan");
    expect(section(container, "Gates")?.textContent).toContain("Waiting on you");
    expect(container.querySelector(".queue-question")?.textContent).toContain("Approve the proposal");
    expect(container.querySelector(".item-callout")).toBeNull();
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
