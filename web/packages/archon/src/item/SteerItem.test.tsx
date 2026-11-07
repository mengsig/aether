// @vitest-environment happy-dom
import { afterEach, beforeEach, test, expect, vi } from "vitest";
import { archonHarness, byText, click, type, type ArchonHarness } from "../queue/test/harness.tsx";
import { demoWorld } from "../queue/fixture/world.ts";
import ItemRoute from "./ItemScreen.tsx";

let h: ArchonHarness | null = null;
beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { h?.stop(); h = null; document.body.innerHTML = ""; vi.useRealTimers(); });

test("shows the stored plan and submits a validated steer", async () => {
  const world = demoWorld();
  const entry = world.items.get("qi_rate_limit");
  if (!entry) throw new Error("missing item");
  entry.detail.item = { ...entry.detail.item as object, model_plan: { build: [{ model: { provider: "codex", model: "astra" }, effort: "high" }] }, budget_usd: 12, stage_timeouts: { build: 600 } };
  h = await archonHarness({ protocol: 24 }, world);
  const container = await h.mount(<ItemRoute params={{ item: "qi_rate_limit" }} />);
  await h.until(() => container.textContent?.includes("codex/astra · high") === true, "stored settings");
  await h.until(() => !(byText(container, "button", "Steer item") as HTMLButtonElement).disabled, "steering permission");
  expect(container.textContent).toContain("Budget: $12");
  expect(container.textContent).toContain("build: 600s");
  await click(h, byText(container, "button", "Steer item"));
  await h.until(() => document.querySelector('select[disabled]') === null, "policy");
  await click(h, byText(document, "dialog button", "Apply changes"));
  expect(document.querySelector('dialog [role="alert"]')?.textContent).toContain("Choose a change");
  const labels = [...document.querySelectorAll<HTMLLabelElement>("dialog label")];
  const field = (label: string) => {
    const node = labels.find((entry) => entry.textContent?.includes(label));
    const element = node?.querySelector("input, textarea, select") ?? (node?.htmlFor ? document.getElementById(node.htmlFor) : null);
    if (!element) throw new Error(`missing ${label}`);
    return element;
  };
  await type(h, field("Stage"), "build");
  await type(h, field("Effort"), "medium");
  await type(h, field("Note"), "  Focus on tests  ");
  await type(h, field("Budget"), "20");
  await type(h, field("Extra time"), "15");
  await click(h, byText(document, "dialog button", "Apply changes"));
  expect(h.faux.commands("update_item")).toEqual([expect.objectContaining({ item: "qi_rate_limit", patch: { model_plan: { build: [{ model: { provider: "codex", model: "astra" }, effort: "medium" }] } } })]);
  expect(h.faux.commands("steer_item")).toEqual([expect.objectContaining({ item: "qi_rate_limit", steer: { stage: "build", note: "Focus on tests", budget_usd: 20, extend_minutes: 15 } })]);
});


test("updates the priority of a queued item", async () => {
  h = await archonHarness({ protocol: 24 });
  const container = await h.mount(<ItemRoute params={{ item: "qi_rate_limit" }} />);
  await h.until(() => container.querySelector(".item-actions select") !== null, "priority control");
  await h.until(() => !(container.querySelector(".item-actions select") as HTMLSelectElement).disabled, "editing permission");
  const select = container.querySelector(".item-actions select");
  if (!select) throw new Error("missing priority");
  await type(h, select, "high");
  await click(h, byText(container, "button", "Update priority"));
  expect(h.faux.commands("update_item")).toEqual([expect.objectContaining({ item: "qi_rate_limit", patch: { priority: "high" } })]);
});

test.each([{ protocol: 23 as const }, { protocol: 24 as const, role: "observer" as const }])("refuses edits without a protocol 24 operator", async (options) => {
  h = await archonHarness(options);
  const container = await h.mount(<ItemRoute params={{ item: "qi_rate_limit" }} />);
  await h.until(() => container.textContent?.includes("Steer item") === true, "steering control");
  expect((byText(container, "button", "Steer item") as HTMLButtonElement).disabled).toBe(true);
  expect(container.textContent).toMatch(/require protocol 24|only observes/);
});

test("disables steering with a visible reason when the daemon refuses the policy read", async () => {
  h = await archonHarness({ protocol: 24, refuseModelPolicy: true });
  const container = await h.mount(<ItemRoute params={{ item: "qi_rate_limit" }} />);
  await h.until(() => container.textContent?.includes("Steering is unavailable") === true, "the refusal");
  expect((byText(container, "button", "Steer item") as HTMLButtonElement).disabled).toBe(true);
  expect((byText(container, "button", "Update priority") as HTMLButtonElement).disabled).toBe(true);
  expect(container.textContent).toContain("model policy is unavailable");
});
