// @vitest-environment happy-dom
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, expect, test, vi } from "vitest";
import { SETTLE_MS } from "./data.ts";
import { connected, type Harness } from "./fixture/harness.test-util.tsx";
import { LESSON, VOTE_QUESTION, scenario } from "./fixture/scenario.ts";
import { ProblemCard } from "./ProblemCard.tsx";
import { QuestionCard } from "./QuestionCard.tsx";
import SessionRoute from "./SessionScreen.tsx";

let harness: Harness | null = null;
afterEach(() => { harness?.unmount(); harness = null; });

test("teacher and learner bubbles use shared block rendering", async () => {
  const h = await connected(scenario(true, { transport: "ipc", transcripts: {
    [LESSON.agent]: [{ role: "user", content: [{ type: "text", text: "My *reasoning*." }] }],
  } }), vi);
  harness = h;
  location.hash = `#/learn/sessions/${LESSON.id}`;
  const root = await h.mount(<SessionRoute params={{ id: LESSON.id }} />);
  await h.settle(SETTLE_MS);
  h.daemon.teach(LESSON.id, "# Teacher\n\n**Read** this.");
  await h.settle(SETTLE_MS);
  expect(root.querySelector('.learn-bubble[data-role="teacher"] .md h3')?.textContent).toBe("Teacher");
  expect(root.querySelector('.learn-bubble[data-role="learner"] .md em')?.textContent).toBe("reasoning");
});

test("question prompts, option labels, answer keys and explanations render at the right level", () => {
  const question = { ...VOTE_QUESTION.question, prompt: "# Choose\n\n**Think** first.", options: [{ value: "a", label: "**One**" }, { value: "b", label: "`Two`" }] };
  const root = document.createElement("div");
  root.innerHTML = renderToStaticMarkup(<QuestionCard question={question} answer={undefined} result={undefined} open />);
  expect(root.querySelector(".learn-q-prompt .md h3")?.textContent).toBe("Choose");
  expect(root.querySelector(".learn-option .md-inline strong")?.textContent).toBe("One");
  expect(root.querySelector(".learn-option p,.learn-option div")).toBeNull();
  root.innerHTML = renderToStaticMarkup(<QuestionCard question={question} answer={undefined} open={false}
    result={{ question: question.id, correct: true, answer_key: ["a"], explanation: "## Why\n\n- **Reason**" }} />);
  expect(root.querySelector(".learn-reveal .md-inline strong")?.textContent).toBe("One");
  expect(root.querySelector(".learn-reveal-why .md li strong")?.textContent).toBe("Reason");
});

test("problem statements and concluded solutions use block rendering", () => {
  const root = document.createElement("div");
  root.innerHTML = renderToStaticMarkup(<ProblemCard active={false}
    problem={{ id: "p", mode: "recall", posed_at: "", targets: [], statement: "# Explain\n\n**Energy**" }}
    conclusion={{ outcome: "solved", rung: 0, miss_class: null, retest: false, solution: "## Solution\n\n- First\n- Second" }} />);
  expect(root.querySelector(".learn-q-prompt .md h3")?.textContent).toBe("Explain");
  expect(root.querySelector(".learn-reveal .md h4")?.textContent).toBe("Solution");
  expect(root.querySelectorAll(".learn-reveal .md li")).toHaveLength(2);
});
