// @vitest-environment happy-dom
// Every combination of an item's execution, the page's issue or PR state and the item's delivered PR state,
// rendered: "succeeded" (an item), "merged" (a PR) and "closed" (an issue or PR) are never shown as one another.

import type { ArchonItem, Stamped } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, test } from "vitest";
import { EXECUTION_LABELS, githubFact } from "./facts.ts";
import { joinPage } from "./join.ts";
import { link, object, ref } from "./fixture/world.ts";
import { FactCard, ItemCard } from "./Sections.tsx";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const KINDS = Object.keys(EXECUTION_LABELS) as archon.QueueStateKind[];
const PR_STATES: readonly archon.PrState[] = ["open", "draft", "merged", "closed"];
interface Page { readonly kind: archon.GithubKind; readonly state: string; readonly draft?: boolean; readonly reason?: string; readonly expect: string; readonly status: string }
const PAGES: readonly Page[] = [
  { kind: "issue", state: "open", expect: "Open", status: "issue-open" },
  { kind: "issue", state: "closed", reason: "completed", expect: "Closed", status: "issue-closed" },
  { kind: "issue", state: "closed", reason: "not_planned", expect: "Closed", status: "issue-closed" },
  { kind: "pull_request", state: "open", expect: "Open", status: "pr-open" },
  { kind: "pull_request", state: "open", draft: true, expect: "Draft", status: "pr-draft" },
  { kind: "pull_request", state: "merged", expect: "Merged", status: "pr-merged" },
  { kind: "pull_request", state: "closed", expect: "Closed", status: "pr-closed" },
];
const WORDS: Readonly<Record<archon.PrState, string>> = { open: "Open", draft: "Draft", merged: "Merged", closed: "Closed" };

function stateOf(kind: archon.QueueStateKind): archon.QueueState {
  switch (kind) {
    case "running": return { state: "running", run: "run_1" };
    case "blocked": return { state: "blocked", reason: "A dependency failed" };
    case "succeeded": return { state: "succeeded", run: "run_1", summary: "Done and delivered" };
    case "failed": return { state: "failed", error: "The build broke" };
    case "cancelled": return { state: "cancelled", reason: null };
    default: return { state: kind };
  }
}

function scenario(kind: archon.QueueStateKind, page: Page, delivered: archon.PrState) {
  const at = ref("acme/site", 10, page.kind) as unknown as archon.GithubRef;
  const pr = ref("acme/site", 11, "pull_request") as unknown as archon.GithubRef;
  const itemPr: archon.PrRef = page.kind === "pull_request" ? { ...at, state: "merged" } : { ...pr, state: delivered };
  const summary = { id: "qi_1", title: "The item", state: stateOf(kind), pr: itemPr } as unknown as ArchonItem["summary"];
  const items: Record<string, ArchonItem> = { qi_1: { summary, state: kind, asOf: 1 } };
  const cached = [object(at as never, "The object", page.state, { ...(page.draft ? { draft: true } : {}), ...(page.reason ? { state_reason: page.reason } : {}) })] as unknown as archon.GithubObject[];
  const links = [link("qi_1", at as never, page.kind === "issue" ? "origin" : "delivered")] as unknown as archon.GithubLink[];
  const itemLinks: Record<string, Stamped> = { qi_1: { value: { links: [], objects: [] }, asOf: 1 } };
  return joinPage({ page: { host: "github.com", repo: "acme/site", number: 10 }, cached, links, items, details: {}, itemLinks });
}

let root: ReturnType<typeof createRoot> | null = null;
afterEach(() => {
  if (root) act(() => root?.unmount());
  root = null;
  document.body.innerHTML = "";
});

function render(kind: archon.QueueStateKind, page: Page, delivered: archon.PrState): HTMLElement {
  const view = scenario(kind, page, delivered);
  const [item] = view.items;
  if (!item) throw new Error("the scenario links one item");
  const container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() => root?.render(<><FactCard fact={githubFact(view.kind, view.object, view.recorded)} /><ul><ItemCard view={item} /></ul></>));
  return container;
}

const cards = (container: HTMLElement) => [...container.querySelectorAll(".gh-fact")].map((card) => ({
  fact: card.getAttribute("data-fact"), text: card.querySelector(".ui-pill")?.textContent ?? "", status: card.querySelector(".ui-pill")?.getAttribute("data-status"),
}));

describe("states kept apart", () => {
  const combinations = KINDS.flatMap((kind) => PAGES.flatMap((page) => (page.kind === "issue" ? PR_STATES : (["merged"] as const)).map((delivered) => ({ kind, page, delivered }))));

  test("covers every combination", () => {
    expect(combinations).toHaveLength(KINDS.length * (3 * PR_STATES.length + 4));
  });

  test.each(combinations)("item $kind · $page.kind $page.expect · delivered PR $delivered", ({ kind, page, delivered }) => {
    const shown = cards(render(kind, page, delivered));
    const own = shown.find((card) => card.fact === (page.kind === "issue" ? "Issue" : "Pull request"));
    const execution = shown.find((card) => card.fact === "Execution");
    expect(own).toEqual({ fact: page.kind === "issue" ? "Issue" : "Pull request", text: page.expect, status: page.status });
    expect(execution).toEqual({ fact: "Execution", text: EXECUTION_LABELS[kind], status: kind });
    const deliveredCard = shown.find((card) => card.fact === "Delivered PR");
    if (page.kind === "issue") expect(deliveredCard).toEqual({ fact: "Delivered PR", text: `#11 · ${WORDS[delivered]}`, status: `pr-${delivered}` });
    else expect(deliveredCard).toBeUndefined();
    for (const card of shown) {
      const fromItem = card.fact === "Execution";
      expect(/Succeeded/.test(card.text)).toBe(fromItem && kind === "succeeded");
      if (/Merged/.test(card.text)) expect(card.fact === "Pull request" || card.fact === "Delivered PR").toBe(true);
      if (/Closed/.test(card.text)) expect(fromItem).toBe(false);
      expect(card.status?.startsWith("pr-") || card.status?.startsWith("issue-")).toBe(!fromItem);
    }
  });
});
