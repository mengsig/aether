// @vitest-environment happy-dom
// The session screen against the faux daemon: questions of every kind with their answers sealed until
// recorded, the pending state while the teacher works, and the confirm step before deleting.

import { afterEach, describe, expect, test, vi } from "vitest";
import { $, byText, click, connected, must, type, type Harness } from "./fixture/harness.test-util.tsx";
import { GOAL_QUESTION, LESSON, MULTI_QUESTION, RECALL_QUESTION, scenario, VOTE_QUESTION } from "./fixture/scenario.ts";
import SessionRoute, { STALL_MS } from "./SessionScreen.tsx";
import { SETTLE_MS } from "./data.ts";

let harness: Harness | null = null;
afterEach(() => {
  harness?.unmount();
  harness = null;
});

async function openLesson(): Promise<{ h: Harness; root: HTMLElement }> {
  const h = await connected(scenario(true, { transport: "ipc" }), vi);
  harness = h;
  location.hash = `#/learn/sessions/${LESSON.id}`;
  const root = await h.mount(<SessionRoute params={{ id: LESSON.id }} />);
  await h.settle(SETTLE_MS);
  expect(root.textContent).toContain(LESSON.goal);
  return { h, root };
}

const answers = (h: Harness) => h.daemon.sent.filter((command) => command.cmd === "noesis_answer").map((command) => command.answer);

describe("questions", () => {
  test.each([
    { values: ["0", "1"], reversed: false, correct: true },
    { values: ["0", "1"], reversed: true, correct: false },
    { values: ["1", "2"], reversed: false, correct: false },
    { values: ["1", "2"], reversed: true, correct: true },
  ] as const)("choice values follow the displayed labels: $values, reversed=$reversed, correct=$correct", async ({ values, reversed, correct }) => {
    const { h, root } = await openLesson();
    const options = [{ value: values[0], label: "Square" }, { value: values[1], label: "Circle" }];
    if (reversed) options.reverse();
    h.daemon.pose(LESSON.id, { ...VOTE_QUESTION, key: [values[0]], explanation: "A square has four equal sides.",
      question: { ...VOTE_QUESTION.question, options, prompt: "Which shape has four equal sides?" } });
    await h.settle(SETTLE_MS);
    const card = $(root, ".learn-q[data-state=open]");
    expect(card.textContent).not.toContain("A square has four equal sides.");
    expect(card.querySelector(".learn-reveal")).toBeNull();
    await click(h, $(byText(card, "label", correct ? "Square" : "Circle"), "input"));
    await click(h, byText(card, "button", "Answer"));
    await h.settle(SETTLE_MS);
    expect(answers(h)[0]).toMatchObject({ choices: [correct ? values[0] : values[1]] });
    const reveal = root.querySelector(".learn-reveal");
    expect(reveal?.textContent).toContain(correct ? "✓ Correct" : "✗ Not quite");
    expect(reveal?.textContent).toContain(correct ? "Answer: Square" : "Correct answer: Square");
  });

  test("a choice question's answer and explanation stay out of the page and the store until the answer is recorded", async () => {
    const { h, root } = await openLesson();
    h.daemon.pose(LESSON.id, VOTE_QUESTION);
    await h.settle(SETTLE_MS);
    const card = root.querySelector<HTMLElement>(".learn-q[data-state=open]");
    expect(card?.textContent).toContain(VOTE_QUESTION.question.prompt);
    expect(root.textContent).not.toContain(VOTE_QUESTION.explanation);
    expect(root.querySelector(".learn-reveal")).toBeNull();
    expect(JSON.stringify(h.app.store.get())).not.toContain(VOTE_QUESTION.explanation);

    await click(h, $(byText(must(card, "the open question"), "label", "Refuses: one vote per term"), "input"));
    await click(h, byText(must(card, "the open question"), "button", "Answer"));
    await h.settle(SETTLE_MS);
    expect(answers(h)).toEqual([{ dont_know: false, choices: ["b"], confidence: 50 }]);
    const reveal = root.querySelector(".learn-reveal");
    expect(reveal?.textContent).toContain("✓ Correct");
    expect(reveal?.textContent).toContain("Answer: Refuses: one vote per term");
    expect(reveal?.textContent).toContain(VOTE_QUESTION.explanation);
    expect(root.querySelector(".learn-q[data-state=correct]")).not.toBeNull();
  });

  test("a multiple-choice question takes several options and says when the answer is wrong", async () => {
    const { h, root } = await openLesson();
    h.daemon.pose(LESSON.id, MULTI_QUESTION);
    await h.settle(SETTLE_MS);
    const card = $(root, ".learn-q[data-state=open]");
    expect(card.querySelectorAll("input[type=checkbox]")).toHaveLength(3);
    await click(h, $(byText(card, "label", "The last log index"), "input"));
    await click(h, $(byText(card, "label", "The candidate's node id"), "input"));
    await click(h, byText(card, "[role=radio]", "90%"));
    await click(h, byText(card, "button", "Answer"));
    await h.settle(SETTLE_MS);
    expect(answers(h)).toEqual([{ dont_know: false, choices: ["i", "n"], confidence: 90 }]);
    expect(root.querySelector(".learn-reveal")?.textContent).toContain("✗ Not quite");
    expect(root.querySelector(".learn-reveal")?.textContent).toContain("The last log entry's term, The last log index");
  });

  test("a recall question takes text with a confidence, and I don't know sends no confidence", async () => {
    const { h, root } = await openLesson();
    h.daemon.pose(LESSON.id, RECALL_QUESTION);
    await h.settle(SETTLE_MS);
    let card = $(root, ".learn-q[data-state=open]");
    expect(card.querySelector("[data-testid=learn-answer]")).not.toBeNull();
    expect(card.textContent).toContain("How sure are you?");
    await click(h, byText(card, "button", "I don't know"));
    await h.settle(SETTLE_MS);
    expect(answers(h)).toEqual([{ dont_know: true }]);
    expect(root.querySelector(".learn-reveal")?.textContent).toContain(RECALL_QUESTION.explanation);

    h.daemon.pose(LESSON.id, { ...RECALL_QUESTION, question: { ...RECALL_QUESTION.question, id: "q_term_2" } });
    await h.settle(SETTLE_MS);
    card = $(root, ".learn-q[data-state=open]");
    await type(h, $(card, "[data-testid=learn-answer]"), "  It steps down to follower.  ");
    await click(h, byText(card, "button", "Answer"));
    expect(answers(h)[1]).toEqual({ dont_know: false, text: "It steps down to follower.", confidence: 50 });
  });

  test("a goal question is ungraded: no confidence and no I don't know", async () => {
    const { h, root } = await openLesson();
    h.daemon.pose(LESSON.id, GOAL_QUESTION);
    await h.settle(SETTLE_MS);
    const card = $(root, ".learn-q[data-state=open]");
    expect(card.textContent).toContain("Your goal");
    expect(card.textContent).not.toContain("How sure are you?");
    expect(card.textContent).not.toContain("I don't know");
    await type(h, $(card, "textarea"), "Explain an election to a colleague");
    await click(h, byText(card, "button", "Answer"));
    expect(answers(h)).toEqual([{ dont_know: false, text: "Explain an election to a colleague" }]);
  });

  test("the composer waits while a question is open", async () => {
    const { h, root } = await openLesson();
    h.daemon.pose(LESSON.id, VOTE_QUESTION);
    await h.settle(SETTLE_MS);
    const composer = ($(root, "[data-testid=learn-reply]") as HTMLTextAreaElement);
    expect(composer.disabled).toBe(true);
    expect(composer.placeholder).toContain("Answer the question above");
  });
});

describe("teaching turns", () => {
  test("sending shows the teacher working until the committed reply arrives", async () => {
    const { h, root } = await openLesson();
    await type(h, $(root, "[data-testid=learn-reply]"), "What stops two leaders?");
    await click(h, byText(root, "button", "Send"));
    expect(h.daemon.sent.filter((c) => c.cmd === "noesis_send").map((c) => c.text)).toEqual(["What stops two leaders?"]);
    expect(root.querySelector(".learn-working")?.textContent).toContain("Your teacher is writing");
    expect(($(root, "[data-testid=learn-reply]") as HTMLTextAreaElement).disabled).toBe(true);

    h.daemon.teach(LESSON.id, "Each server votes once per term, and a leader needs a majority.");
    await h.settle(SETTLE_MS);
    expect(root.querySelector(".learn-working")).toBeNull();
    expect(root.textContent).toContain("Each server votes once per term");
  });

  test("a turn that never answers turns into a calm note with a way to check again", async () => {
    const { h, root } = await openLesson();
    await type(h, $(root, "[data-testid=learn-reply]"), "Hello?");
    await click(h, byText(root, "button", "Send"));
    await h.settle(STALL_MS);
    const note = root.querySelector(".learn-callout[role=status]");
    expect(note?.textContent).toContain("No reply yet");
    const reads = h.daemon.names().filter((cmd) => cmd === "noesis_get_session").length;
    await click(h, byText(root, "button", "Check again"));
    await h.settle(SETTLE_MS);
    expect(h.daemon.names().filter((cmd) => cmd === "noesis_get_session").length).toBe(reads + 1);
  });
});

describe("deleting a session", () => {
  test("asks first, says the transcript stays, and deletes only on confirm", async () => {
    const { h, root } = await openLesson();
    await click(h, byText(root, "button", "Delete session…"));
    const dialog = document.querySelector("dialog[open]");
    expect(dialog?.textContent).toContain("stays in Aether history");
    expect(h.daemon.names()).not.toContain("noesis_delete_session");

    await click(h, byText(must(dialog, "the dialog"), "button", "Keep it"));
    expect(document.querySelector("dialog[open]")).toBeNull();
    expect(h.daemon.names()).not.toContain("noesis_delete_session");

    await click(h, byText(root, "button", "Delete session…"));
    await click(h, byText($(document, "dialog[open]"), "button", "Delete session"));
    expect(h.daemon.sent.filter((c) => c.cmd === "noesis_delete_session")).toMatchObject([{ session: LESSON.id, graph: "preserve" }]);
    expect(location.hash).toBe("#/learn");
    expect(h.app.store.get().aether["aether:faux-noesis"]?.learn.sessions[LESSON.id]).toBeUndefined();
  });
});
