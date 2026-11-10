// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from "vitest";
import { $, byText, click, connected, type Harness } from "./fixture/harness.test-util.tsx";
import { LESSON, PLAN, scenario, VOTE_QUESTION } from "./fixture/scenario.ts";
import SessionRoute from "./SessionScreen.tsx";
import { SETTLE_MS } from "./data.ts";

let harness: Harness | null = null;
afterEach(() => {
  harness?.unmount();
  harness = null;
});

test("a complete plan waits for the open check to be answered before showing completion", async () => {
  const plan = { ...PLAN, nodes: PLAN.nodes.map((node) => ({ ...node, status: "visited" as const })) };
  const h = await connected(scenario(true, { transport: "ipc", sessions: [{ ...LESSON, plan }] }), vi);
  harness = h;
  h.daemon.pose(LESSON.id, VOTE_QUESTION);
  location.hash = `#/learn/sessions/${LESSON.id}`;
  const root = await h.mount(<SessionRoute params={{ id: LESSON.id }} />);
  await h.settle(SETTLE_MS);

  const question = $(root, ".learn-q[data-state=open]");
  expect(question.textContent).toContain(VOTE_QUESTION.question.prompt);
  expect(root.querySelector("[aria-label='Lesson complete']")).toBeNull();
  expect(root.querySelector(".learn-plan")?.textContent).toContain(`${plan.nodes.length} of ${plan.nodes.length} done`);

  await click(h, $(byText(question, "label", "Refuses: one vote per term"), "input"));
  await click(h, byText(question, "button", "Answer"));
  await h.settle(SETTLE_MS);
  expect(root.querySelector(".learn-q[data-state=open]")).toBeNull();
  expect(root.querySelector("[aria-label='Lesson complete']")?.textContent).toContain("You finished this lesson");
  expect(root.querySelector(".learn-plan")?.textContent).toContain(`${plan.nodes.length} of ${plan.nodes.length} done`);
});

test("an incomplete plan does not show completion", async () => {
  const h = await connected(scenario(true, { transport: "ipc" }), vi);
  harness = h;
  location.hash = `#/learn/sessions/${LESSON.id}`;
  const root = await h.mount(<SessionRoute params={{ id: LESSON.id }} />);
  await h.settle(SETTLE_MS);
  const visited = PLAN.nodes.filter((node) => node.status === "visited").length;
  expect(visited).toBeLessThan(PLAN.nodes.length);
  expect(root.querySelector("[aria-label='Lesson complete']")).toBeNull();
  expect(root.querySelector(".learn-plan")?.textContent).toContain(`${visited} of ${PLAN.nodes.length} done`);
});
