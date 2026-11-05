// @vitest-environment happy-dom
// Acceptance: mastery is drawn with the `--mastery-*` tokens and always named in text, so colour is
// never the only signal: on the badge, on "my understanding" and in the knowledge graph.

import { afterEach, describe, expect, test, vi } from "vitest";
import { $, connected, must, type Harness } from "./fixture/harness.test-util.tsx";
import { CONCEPTS, GRAPH, scenario } from "./fixture/scenario.ts";
import { knowledgeGraph, knowledgeModel, masteryByProposition } from "./graph-model.ts";
import { SETTLE_MS } from "./data.ts";
import { MASTERY, MASTERY_ORDER } from "./model.ts";
import UnderstandingRoute from "./UnderstandingScreen.tsx";

/** The stylesheet as written: vitest blanks CSS imports, and packages carry no Node types. */
function readStylesheet(): string {
  const node = (globalThis as unknown as { process: { getBuiltinModule(id: "node:fs"): { readFileSync(path: string, encoding: "utf8"): string } } }).process;
  const dir = (import.meta as ImportMeta & { dirname: string }).dirname;
  return node.getBuiltinModule("node:fs").readFileSync(`${dir}/learn.css`, "utf8");
}

const css = readStylesheet();

let harness: Harness | null = null;
afterEach(() => {
  harness?.unmount();
  harness = null;
});

describe("mastery tokens and labels", () => {
  test("each state maps to its own --mastery-* token in the stylesheet, and no raw colour stands in", () => {
    for (const state of MASTERY_ORDER) {
      expect(css).toContain(`[data-mastery="${state}"] { --learn-mastery: var(--mastery-${state}); }`);
    }
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b|rgb\(|hsl\(/i);
  });

  test("each state has a distinct text label and a distinct mark shape", () => {
    const labels = MASTERY_ORDER.map((state) => MASTERY[state].label);
    expect(new Set(labels).size).toBe(MASTERY_ORDER.length);
    for (const state of MASTERY_ORDER.filter((s) => s !== "demonstrated")) {
      expect(css).toContain(`.learn-mastery[data-mastery="${state}"] .learn-mastery-mark`);
    }
  });

  test("my understanding names every concept's state in text beside its token colour", async () => {
    harness = await connected(scenario(true), vi);
    const root = await harness.mount(<UnderstandingRoute />);
    await harness.settle(SETTLE_MS);
    const rows = [...root.querySelectorAll<HTMLElement>(".learn-concept")];
    expect(rows).toHaveLength(CONCEPTS.length);
    for (const row of rows) {
      const state = row.dataset.mastery as keyof typeof MASTERY;
      const badge = row.querySelector<HTMLElement>(".learn-mastery");
      expect(badge?.dataset.mastery).toBe(state);
      expect(badge?.textContent).toBe(MASTERY[state].label);
      expect(row.querySelector("meter")?.getAttribute("data-mastery")).toBe(state);
    }
    const shown = new Set(rows.map((row) => row.dataset.mastery));
    expect(shown).toEqual(new Set(MASTERY_ORDER));
    const summary = $(root, "[aria-label='Concepts by state']");
    for (const state of MASTERY_ORDER) expect(summary.textContent).toContain(MASTERY[state].label);
    expect(root.querySelector("[aria-label=Misconception]")?.textContent).toContain("Thinks a server can vote again");
  });

  test("graph nodes carry their mastery as a status the canvas colours with --mastery-*", () => {
    const graph = must(knowledgeGraph(GRAPH as unknown as Record<string, unknown>), "the graph");
    const model = knowledgeModel(graph, masteryByProposition(CONCEPTS));
    const status = Object.fromEntries(model.nodes.map((node) => [node.id, node.status]));
    expect(status.p_vote).toBe("mastery-misconception");
    expect(status.p_term).toBe("mastery-demonstrated");
    expect(status.p_step_down).toBe("mastery-unseen");
  });
});
