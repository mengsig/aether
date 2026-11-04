// @vitest-environment happy-dom
// The session's plan tree and practice problems against the faux daemon: the plan in prerequisite
// order with a word for each step, and a problem's solution sealed until Noesis concludes it.

import type { aether } from "@aether-ui/protocol";
import { afterEach, describe, expect, test, vi } from "vitest";
import { SETTLE_MS } from "./data.ts";
import { $, connected, must, type Harness } from "./fixture/harness.test-util.tsx";
import { LESSON, PLAN, PRACTICE, scenario } from "./fixture/scenario.ts";
import { planRows } from "./model.ts";
import SessionRoute from "./SessionScreen.tsx";

let harness: Harness | null = null;
afterEach(() => {
  harness?.unmount();
  harness = null;
});

async function openSession(session: aether.NoesisSession): Promise<{ h: Harness; root: HTMLElement }> {
  const h = await connected(scenario(true, { transport: "ipc" }), vi);
  harness = h;
  location.hash = `#/learn/sessions/${session.id}`;
  const root = await h.mount(<SessionRoute params={{ id: session.id }} />);
  await h.settle(SETTLE_MS);
  expect(root.textContent).toContain(session.goal);
  return { h, root };
}

describe("plan tree", () => {
  test("orders steps by prerequisite, indents by depth, and labels each step in words", async () => {
    const { root } = await openSession(LESSON);
    const plan = $(root, "section[aria-label=Plan]");
    const steps = [...plan.querySelectorAll<HTMLElement>(".learn-plan-step")];
    expect(steps.map((step) => step.querySelector(".learn-plan-title")?.textContent)).toEqual([
      "Terms and heartbeats", "Randomised election timeouts", "Who grants a vote", "Split votes and retries", "The up-to-date log rule",
    ]);
    expect(steps.map((step) => step.querySelector(".learn-plan-status")?.textContent)).toEqual(["Done", "Done", "Now", "Later", "Later"]);
    expect(steps.map((step) => step.getAttribute("aria-current"))).toEqual([null, null, "step", null, null]);
    expect(steps.map((step) => step.style.marginInlineStart)).toEqual([0, 1, 2, 2, 3].map((depth) => `calc(${depth} * var(--space-4))`));
    expect(plan.textContent).toContain("2 of 5 done");
  });

  test("a plan with a cycle or a dangling edge still places every step exactly once", () => {
    const tangled: aether.PlanTree = {
      ...PLAN,
      edges: [...PLAN.edges, { from: "pn_split", to: "pn_timeout" }, { from: "pn_log", to: "pn_missing" }],
    };
    const ids = planRows(tangled).map((row) => row.node.id);
    expect([...ids].sort()).toEqual(PLAN.nodes.map((node) => node.id).sort());
    expect(ids[0]).toBe("pn_terms");
  });
});

describe("practice", () => {
  const SOLUTION = "Both time out again at random; the one that times out first gathers three votes and wins the next term.";
  const problemCard = (root: HTMLElement) => $(root, "article[aria-label='Practice problem']");

  test("the active problem shows its statement, and its solution stays out of the page and the store until it concludes", async () => {
    const { h, root } = await openSession(PRACTICE);
    const problem = must(PRACTICE.active_problem, "the practice session's problem");
    let card = problemCard(root);
    expect(card.dataset.state).toBe("open");
    expect(card.textContent).toContain("Practice · Put together");
    expect(card.textContent).toContain(problem.statement);
    expect(card.querySelector(".learn-reveal")).toBeNull();
    expect(JSON.stringify(h.app.store.get())).not.toContain(SOLUTION);

    h.daemon.conclude(PRACTICE.id, { problem: problem.id, outcome: "solved_with_hints", rung: 2, miss_class: "cue", retest: true, solution: SOLUTION });
    await h.settle(SETTLE_MS);
    card = problemCard(root);
    expect(card.dataset.state).toBe("answered");
    const reveal = must(card.querySelector(".learn-reveal"), "the conclusion");
    expect(reveal.textContent).toContain("Solved with hints");
    expect(reveal.textContent).toContain("Hints used: 2 of 5 · Missed the cue · Comes back for another try");
    expect(reveal.textContent).toContain(`Solution: ${SOLUTION}`);
  });

  test("a problem posed mid-session joins the conversation open, with no solution", async () => {
    const { h, root } = await openSession(LESSON);
    expect(root.querySelector("article[aria-label='Practice problem']")).toBeNull();
    h.daemon.problem(LESSON.id, { id: "pb_vote", mode: "recall", posed_at: "", targets: ["c_vote"], statement: "Why can a server vote only once per term?" });
    await h.settle(SETTLE_MS);
    const card = problemCard(root);
    expect(card.dataset.state).toBe("open");
    expect(card.textContent).toContain("Practice · Recall");
    expect(card.textContent).toContain("Why can a server vote only once per term?");
    expect(card.textContent).not.toContain("Solution");
  });
});
