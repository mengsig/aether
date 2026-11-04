// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { archonHarness, byText, click, CONN, type, type ArchonHarness } from "./test/harness.tsx";
import { QueueScreen } from "./QueueScreen.tsx";

let h: ArchonHarness | null = null;
beforeEach(() => { vi.useFakeTimers({ now: Date.parse("2026-11-04T10:00:00Z") }); });
afterEach(() => {
  h?.stop();
  h = null;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

const total = (container: HTMLElement) => container.querySelector(".queue-total")?.textContent ?? "";
const titles = (container: HTMLElement) => [...container.querySelectorAll(".queue-title")].map((node) => node.textContent);
const chip = (container: HTMLElement, label: string) => byText(container, ".queue-chip", new RegExp(`^${label} \\d+$`));

async function queue(options: Parameters<typeof archonHarness>[0] = { protocol: 24 }) {
  h = await archonHarness(options);
  const container = await h.mount(<QueueScreen />);
  await h.until(() => container.querySelector(".queue-summary") !== null, "the status header");
  return { h, container };
}

describe("the queue list", () => {
  test("orders active work by who it waits on, then priority, with waiting reasons and spend", async () => {
    const { container } = await queue();
    expect(total(container)).toBe("4 of 7 items");
    expect(titles(container)).toEqual(["Shorten the pairing code", "Refresh the landing page copy", "Ship dark mode for the docs", "Rate-limit the claim endpoint"]);
    const pairing = container.querySelector('[data-item="qi_pairing"]');
    expect(pairing?.querySelector(".ui-pill")?.textContent).toBe("Needs you");
    expect(pairing?.querySelector(".queue-note")?.textContent).toBe("Approve the proposal to shorten pairing codes to eight characters?");
    const limit = container.querySelector('[data-item="qi_rate_limit"]');
    expect(limit?.querySelector(".queue-note")?.textContent).toBe("Waiting for a free agent slot (4 of 4 busy)");
    expect(limit?.textContent).toContain("Critical");
    expect(container.querySelector('[data-item="qi_dark_mode"] .queue-spend')?.textContent).toBe("$3.18");
  });

  test("counts each state chip within the other filters, and filters by state, text, repository, pipeline and priority", async () => {
    const { h, container } = await queue();
    expect(["Active", "Waiting on you", "Held up", "Done", "Failed", "All"].map((label) => chip(container, label).textContent))
      .toEqual(["Active 4", "Waiting on you 2", "Held up 3", "Done 2", "Failed 1", "All 7"]);
    await click(h, chip(container, "Done"));
    expect(titles(container)).toEqual(["Write the 1.5 changelog", "Remove the legacy flag"]);
    await click(h, chip(container, "All"));
    await type(h, container.querySelector('input[type="search"]') as Element, "hero");
    expect(titles(container)).toEqual(["Compress the hero images"]);
    await type(h, container.querySelector('input[type="search"]') as Element, "");
    const [repo, pipeline, priority] = [...container.querySelectorAll(".queue-selects select")];
    await type(h, repo as Element, "repo_api");
    expect(chip(container, "All").textContent).toBe("All 2");
    await type(h, priority as Element, "critical");
    expect(titles(container)).toEqual(["Rate-limit the claim endpoint"]);
    await type(h, priority as Element, "");
    await type(h, repo as Element, "");
    await type(h, pipeline as Element, "direct");
    expect(titles(container)).toEqual(["Refresh the landing page copy", "Remove the legacy flag"]);
  });

  test("keeps the execution state apart from the issue and PR", async () => {
    const { h, container } = await queue();
    await click(h, chip(container, "Done"));
    const changelog = container.querySelector('[data-item="qi_changelog"]');
    expect(changelog?.querySelector(".ui-pill")?.textContent).toBe("Succeeded");
    expect([...(changelog?.querySelectorAll(".ui-badge") ?? [])].map((badge) => badge.textContent)).toEqual(["Issue #38", "PR #52 · open"]);
  });

  test("moves a row when the change stream says its state changed", async () => {
    const { h, container } = await queue();
    expect(h.faux.commands("subscribe_changes")).toHaveLength(1);
    h.faux.tell("qi_rate_limit", { subject: "item", item: "qi_rate_limit", state: "running", stage: "plan" });
    await h.until(() => container.querySelector('[data-item="qi_rate_limit"] .ui-pill')?.textContent === "Running", "the live state");
    expect(container.querySelector('[data-item="qi_rate_limit"] .queue-meta')?.textContent).toContain("stage plan");
    expect(h.faux.commands("list_item_summaries")).toHaveLength(1);
    h.faux.tell("qi_rate_limit", { subject: "item", item: "qi_rate_limit", state: "failed" });
    await h.until(() => total(container) === "3 of 7 items", "the failed item to leave the active list");
  });

  test("rereads the summaries at most once a second, even while changes keep coming", async () => {
    const { h } = await queue();
    for (let n = 0; n < 6; n += 1) {
      h.faux.tell("qi_rate_limit", { subject: "item", item: "qi_rate_limit", state: n % 2 ? "queued" : "running" });
      await h.tick(500);
    }
    // A debounce would reread only once the stream stops; a throttle rereads within it.
    expect(h.faux.commands("list_item_summaries").length).toBeGreaterThanOrEqual(3);
  });

  test("drops a stale prompt at once and rereads spend once the changes settle", async () => {
    const { h, container } = await queue();
    const entry = h.world.items.get("qi_pairing");
    if (!entry) throw new Error("the demo world has qi_pairing");
    entry.summary = { ...entry.summary, stage: "build", waiting_reason: null, waiting_on: null, spent_usd: 2.5, updated_at: "2026-11-04T10:00:30Z" };
    const row = () => container.querySelector('[data-item="qi_pairing"]');
    h.faux.tell("qi_pairing", { subject: "item", item: "qi_pairing", state: "running", stage: "build" });
    await h.until(() => row()?.querySelector(".ui-pill")?.textContent === "Running", "the live state");
    expect(row()?.querySelector(".queue-note")).toBeNull();
    expect(chip(container, "Waiting on you").textContent).toBe("Waiting on you 1");
    expect(h.faux.commands("list_item_summaries")).toHaveLength(1);
    await h.until(() => row()?.querySelector(".queue-spend")?.textContent === "$2.50", "the reread spend", 2_000);
    expect(h.faux.commands("list_item_summaries")).toHaveLength(2);
  });
});

describe("the queue's changes", () => {
  test("queues a new item with one idempotency key and opens it", async () => {
    const { h, container } = await queue();
    await click(h, byText(container, "button", "New item"));
    const dialog = document.querySelector("dialog[open]");
    expect(dialog).not.toBeNull();
    await click(h, byText(document, "button", "Queue item"));
    expect(document.querySelector("dialog[open] [role=alert]")?.textContent).toBe("An item needs a title and a brief.");
    const fields = dialog?.querySelectorAll("input, textarea, select") ?? [];
    await type(h, fields[0] as Element, "Add a status page");
    await type(h, fields[1] as Element, "Serve /status with the build id.");
    await type(h, fields[2] as Element, "repo_api");
    await h.until(() => (dialog?.querySelectorAll("select")[1]?.options.length ?? 0) === 3, "the pipeline catalog");
    await type(h, fields[3] as Element, "direct");
    await type(h, fields[4] as Element, "high");
    await type(h, fields[5] as Element, "strict");
    await click(h, byText(document, "button", "Queue item"));
    const [sent] = h.faux.commands("enqueue");
    expect(sent).toMatchObject({ repo: "repo_api", pipeline: "direct", title: "Add a status page", brief: "Serve /status with the build id.", priority: "high", budget_usd: null, autonomy: { review: "strict" } });
    expect(String(sent?.idempotency_key)).toMatch(/^[0-9a-f-]{36}$/);
    expect(h.faux.commands("list_pipelines").at(-1)).toMatchObject({ repo: "repo_api" });
    expect(location.hash).toBe("#/archon/items/qi_new_7");
    await h.until(() => total(container) === "5 of 8 items", "the new item in the queue");
  });

  test("messages the orchestrator", async () => {
    const { h, container } = await queue();
    await click(h, byText(container, "button", "Message Archon"));
    await type(h, document.querySelector("dialog[open] textarea") as Element, "Hold the docs work until Friday.");
    await click(h, byText(document, "dialog[open] button", "Send"));
    expect(h.faux.commands("say")).toEqual([expect.objectContaining({ text: "Hold the docs work until Friday." })]);
    expect(document.querySelector("dialog[open] [role=status]")?.textContent).toBe("Sent to the orchestrator.");
  });

  test("pauses and resumes new work softly", async () => {
    const { h, container } = await queue();
    await click(h, byText(container, "button", "Pause new work"));
    expect(h.faux.commands("pause")).toEqual([expect.objectContaining({ scope: "global", hard: false })]);
    await h.until(() => container.querySelector(".queue-pause strong")?.textContent === "Paused", "the paused status");
    await click(h, byText(container, "button", "Resume new work"));
    expect(h.faux.commands("resume")).toEqual([expect.objectContaining({ scope: "global", hard: false })]);
  });

  test("answers an open question with an option or free text", async () => {
    const { h, container } = await queue();
    const card = container.querySelector(".queue-question");
    expect(card?.textContent).toContain("Approve the proposal");
    expect(card?.querySelector("a")?.getAttribute("href")).toBe("#/archon/items/qi_pairing");
    await click(h, byText(card as Element, "button", "Approve"));
    expect(h.faux.commands("answer")).toEqual([expect.objectContaining({ question: "q_review_gate", text: "Approve" })]);
    await h.until(() => container.querySelector(".queue-question") === null, "the answered question to leave");
  });

  test("rereads the questions when the change stream names one", async () => {
    const { h, container } = await queue();
    h.world.questions.push({ id: "q_new", number: 4, item: null, text: "May the docs work wait until Friday?", options: [], asked_at: "2026-11-04T10:00:00Z" });
    h.faux.tell(null, { subject: "question", question: "q_new", open: true });
    await h.until(() => container.querySelectorAll(".queue-question").length === 2, "the new question", 2_000);
    expect(container.textContent).toContain("May the docs work wait until Friday?");
  });

  test("shows spend against the budgets and a cost report", async () => {
    const { container } = await queue();
    const spend = container.querySelector(".queue-spend-box")?.textContent ?? "";
    expect(spend).toContain("$4.25 of $20.00");
    expect(spend).toContain("$61.90 of $500.00");
    expect(spend).toContain("2026-10-29 to 2026-11-04: $7.49 · site $6.42 · api $1.07");
  });

  test("disables every change for an observer device", async () => {
    const { container } = await queue({ protocol: 24, role: "observer" });
    const controls = ["New item", "Message Archon", "Pause new work", "Approve", "Revise", "Send"].map((label) => byText<HTMLButtonElement>(container, "button", label));
    expect(controls.every((button) => button.disabled && button.title === "This device only observes Archon")).toBe(true);
  });
});

test("names the connection the queue reads", async () => {
  const { h } = await queue();
  expect(h.app.store.get().connections[CONN]?.features).toContain("changes");
});
