// @vitest-environment happy-dom
// A lesson's record against the faux daemon: grades from the session read, every agent's transcript
// without Noesis's own inputs, the teacher's text while it streams, budget refusals and the lesson's end.

import { afterEach, describe, expect, test, vi } from "vitest";
import { $, byText, click, connected, must, type, type Harness } from "./fixture/harness.test-util.tsx";
import type { FauxNoesisOptions } from "./fixture/faux-noesis.ts";
import { LESSON, LESSON_TRANSCRIPT, LESSON_USAGE, MULTI_QUESTION, PLAN, scenario, statusOf, VOTE_QUESTION } from "./fixture/scenario.ts";
import SessionRoute, { STALL_MS } from "./SessionScreen.tsx";
import { SETTLE_MS } from "./data.ts";
import { messageText } from "./model.ts";

let harness: Harness | null = null;
afterEach(() => {
  harness?.unmount();
  harness = null;
});

async function start(extra: Partial<FauxNoesisOptions> = {}): Promise<Harness> {
  const h = await connected(scenario(true, { transport: "ipc", ...extra }), vi);
  harness = h;
  return h;
}

async function open(h: Harness, id = LESSON.id): Promise<HTMLElement> {
  location.hash = `#/learn/sessions/${id}`;
  const root = await h.mount(<SessionRoute params={{ id }} />);
  await h.settle(SETTLE_MS);
  return root;
}

const reveal = (root: HTMLElement) => root.querySelector(".learn-reveal")?.textContent ?? "";
const BUDGET = "noesis.budget.interactive_usd_daily: the interactive budget of $5.00 is spent for today";

describe("grades", () => {
  test("a right answer shows its grade from the session reread, with no answer event", async () => {
    const h = await start();
    h.daemon.answerEvents = false;
    const root = await open(h);
    h.daemon.pose(LESSON.id, VOTE_QUESTION);
    await h.settle(SETTLE_MS);
    const card = $(root, ".learn-q[data-state=open]");
    await click(h, $(byText(card, "label", "Refuses: one vote per term"), "input"));
    await click(h, byText(card, "button", "Answer"));
    await h.settle(SETTLE_MS);
    expect(reveal(root)).toContain("✓ Correct");
    expect(reveal(root)).toContain("Your answer: Refuses: one vote per term · 50% sure");
    expect(reveal(root)).toContain(VOTE_QUESTION.explanation);
  });

  test("a wrong answer shows as wrong, with the correct answer and why", async () => {
    const h = await start();
    h.daemon.answerEvents = false;
    const root = await open(h);
    h.daemon.pose(LESSON.id, MULTI_QUESTION);
    await h.settle(SETTLE_MS);
    const card = $(root, ".learn-q[data-state=open]");
    await click(h, $(byText(card, "label", "The candidate's node id"), "input"));
    await click(h, byText(card, "button", "Answer"));
    await h.settle(SETTLE_MS);
    expect(reveal(root)).toContain("✗ Not quite");
    expect(reveal(root)).toContain("Correct answer: The last log entry's term, The last log index");
    expect(reveal(root)).toContain(`Why: ${MULTI_QUESTION.explanation}`);
  });

  test("a question answered before the page ran shows its answer and grade from the session read alone", async () => {
    const h = await start();
    h.daemon.answeredEarlier(LESSON.id, VOTE_QUESTION, { dont_know: false, choices: ["a"], confidence: 70 });
    const root = await open(h);
    expect(root.textContent).toContain(VOTE_QUESTION.question.prompt);
    expect(reveal(root)).toContain("✗ Not quite");
    expect(reveal(root)).toContain("Your answer: Grants the vote, since the terms match · 70% sure");
    expect(root.querySelector(".learn-q[data-state=open]")).toBeNull();
  });
});

describe("the whole conversation", () => {
  test("a queued turn shows it is waiting for saved settings", async () => {
    const h = await start({ sessions: [LESSON], transcripts: {}, status: { ...statusOf(true), settings_pending: true } });
    const root = await open(h);
    expect(root.textContent).toContain("Waiting for saved settings to take effect");
    expect(root.textContent).not.toContain("Your teacher is writing");
  });

  test("a new session shows writing even when its start preceded the subscription", async () => {
    const h = await start({ sessions: [LESSON], transcripts: {} });
    const root = await open(h);
    expect(root.textContent).toContain("Your teacher is writing…");
    h.daemon.teach(LESSON.id, "Welcome to the lesson.");
    await h.settle(SETTLE_MS);
    expect(root.textContent).not.toContain("Your teacher is writing…");
  });

  test("turn activity shows writing until a buffered reply finishes", async () => {
    const h = await start();
    const root = await open(h);
    h.daemon.turn(LESSON.id, "inv_writing", 1, true);
    await h.settle();
    expect(root.textContent).toContain("Your teacher is writing…");
    expect(($(root, "[data-testid=learn-reply]") as HTMLTextAreaElement).disabled).toBe(true);
    h.daemon.turn(LESSON.id, "inv_writing", 2, false);
    await h.settle();
    expect(root.textContent).not.toContain("Your teacher is writing…");
    expect(($(root, "[data-testid=learn-reply]") as HTMLTextAreaElement).disabled).toBe(false);
  });
  test("the Prober's transcript stays after the hand-off, and Noesis's own inputs never read as the learner's", async () => {
    const h = await start();
    h.daemon.internal(LESSON.id, "NOESIS CONTEXT: the learner studies Raft elections.");
    const root = await open(h);
    h.daemon.handOff(LESSON.id, "teach", "teacher");
    await h.settle(SETTLE_MS);
    h.daemon.teach(LESSON.id, "Now the Teacher: let us look at votes.");
    await h.settle(SETTLE_MS);
    const text = root.textContent ?? "";
    const first = messageText(must(LESSON_TRANSCRIPT[0], "the first message"));
    expect(text).toContain(first);
    expect(text).toContain("Now the Teacher: let us look at votes.");
    expect(text.indexOf(first)).toBeLessThan(text.indexOf("Now the Teacher"));
    expect(text).not.toContain("NOESIS CONTEXT");
    const learner = [...root.querySelectorAll(".learn-bubble[data-role=learner]")].map((bubble) => bubble.textContent);
    expect(learner.every((said) => said?.startsWith("You:") && !said.includes("NOESIS"))).toBe(true);
  });

  test("a fresh page after the hand-off reads both agents' transcripts in order", async () => {
    const h = await start();
    h.daemon.handOff(LESSON.id, "teach", "teacher");
    h.daemon.teach(LESSON.id, "The Teacher picks up here.");
    const root = await open(h);
    const text = root.textContent ?? "";
    expect(text.indexOf(messageText(must(LESSON_TRANSCRIPT[3], "the last message")))).toBeGreaterThan(-1);
    expect(text.indexOf(messageText(must(LESSON_TRANSCRIPT[3], "the last message")))).toBeLessThan(text.indexOf("The Teacher picks up here."));
    expect(h.daemon.names()).toContain("get_transcript_page");
  });

  test("the teacher's text shows while it streams, then once as the committed message", async () => {
    const h = await start();
    const root = await open(h);
    await type(h, $(root, "[data-testid=learn-reply]"), "What stops two leaders?");
    await click(h, byText(root, "button", "Send"));
    h.daemon.stream(LESSON.id, "inv_1", 1, "Each server votes ");
    h.daemon.stream(LESSON.id, "inv_1", 2, "once per term");
    await h.settle();
    expect(root.querySelector(".learn-bubble[data-live]")?.textContent).toContain("Each server votes once per term");
    expect(root.querySelector(".learn-working")).toBeNull();
    h.daemon.teach(LESSON.id, "Each server votes once per term, and a leader needs a majority.", "inv_1");
    await h.settle(SETTLE_MS);
    expect(root.querySelector(".learn-bubble[data-live]")).toBeNull();
    expect((root.textContent ?? "").split("Each server votes once per term")).toHaveLength(2);
  });

  test("the goal heads the lesson, its sources sit in a compact card, and billing says what paid", async () => {
    const context = { title: "PR #42: fix the election timer", items: [{ ref: { kind: "pr" as const, id: "42" }, status: "verified" as const, excerpt: "All checks passed\nVerified: every check run passed." }] };
    const h = await start({ contexts: { [LESSON.id]: context } });
    h.daemon.history.bill(LESSON.id, LESSON_USAGE);
    const root = await open(h);
    expect($(root, ".learn-session-goal").textContent).toBe(LESSON.goal);
    const sources = $(root, ".learn-lesson-sources");
    expect(sources.textContent).toContain("Sources");
    expect(sources.textContent).toContain("Verified");
    expect(sources.textContent).not.toContain("every check run passed");
    expect($(root, "[aria-label='Lesson billing']").textContent).toBe("Ran on your subscription — no per-request charge (≈$0.02 at API list price).");
  });
});

describe("budget", () => {
  test("a stalled teacher offers a retry without a budget warning", async () => {
    const h = await start({ retryTurn: true });
    const root = await open(h);
    h.daemon.stall(LESSON.id, { reason: "engine_stalled", message: "The teacher stopped responding.", at: "2026-01-01T00:00:09Z" });
    await h.settle(SETTLE_MS);
    const note = $(root, ".learn-callout[role=alert]");
    expect(note.textContent).toContain("The teacher stopped responding.");
    expect(root.textContent).not.toContain("Today's teaching budget is spent");
    await click(h, byText(note, "button", "Try again"));
    expect(h.daemon.names()).toContain("noesis_retry_turn");
  });

  test("a budget refusal explains itself at once, never waits, and tries the same turn again", async () => {
    const h = await start();
    h.daemon.refuseTurns = BUDGET;
    const root = await open(h);
    await type(h, $(root, "[data-testid=learn-reply]"), "Go on");
    await click(h, byText(root, "button", "Send"));
    await h.settle();
    const note = $(root, ".learn-callout[data-kind=budget]");
    expect(note.textContent).toContain("Today's teaching budget is spent");
    expect(byText(note, "a", "Open settings").getAttribute("href")).toContain("settings");
    expect(root.querySelector(".learn-working")).toBeNull();
    await h.settle(STALL_MS);
    expect(root.textContent).not.toContain("No reply yet");
    h.daemon.refuseTurns = undefined;
    await click(h, byText(note, "button", "Try again"));
    expect(h.daemon.sent.filter((c) => c.cmd === "noesis_send").map((c) => c.text)).toEqual(["Go on", "Go on"]);
    expect(root.querySelector(".learn-callout[data-kind=budget]")).toBeNull();
  });

  test("an answer the daemon recorded before the budget stopped the next turn ends the wait", async () => {
    const h = await start();
    const root = await open(h);
    h.daemon.pose(LESSON.id, VOTE_QUESTION);
    await h.settle(SETTLE_MS);
    const card = $(root, ".learn-q[data-state=open]");
    await click(h, $(byText(card, "label", "Refuses: one vote per term"), "input"));
    h.daemon.answerEvents = false;
    await click(h, byText(card, "button", "Answer"));
    h.daemon.stall(LESSON.id, { reason: "budget_exhausted", message: "today's Noesis interactive budget is spent", at: "2026-01-01T00:00:09Z" });
    await h.settle(SETTLE_MS);
    expect(root.querySelector(".learn-working")).toBeNull();
    expect($(root, ".learn-callout[data-kind=budget]")).toBeTruthy();
    expect(($(root, "[data-testid=learn-reply]") as HTMLTextAreaElement).disabled).toBe(true);
  });

  test("a day whose teaching budget is spent pauses the lesson and says how much", async () => {
    const status = statusOf(true);
    const h = await start({ status: { ...status, settings: { ...status.settings, budget: { ...status.settings.budget, interactive_usd_daily: 1 } } } });
    const root = await open(h);
    expect($(root, ".learn-callout[data-kind=budget]").textContent).toContain("You have used $1.85 of your $1.00 daily teaching budget.");
    expect(($(root, "[data-testid=learn-reply]") as HTMLTextAreaElement).disabled).toBe(true);
    expect(($(root, "[data-testid=learn-reply]") as HTMLTextAreaElement).placeholder).toContain("paused");
  });

  test("a session the daemon stalled on the budget says so even before today's spend is reread", async () => {
    const stalled = { reason: "budget_exhausted" as const, message: "today's Noesis interactive budget is spent", at: LESSON.updated_at };
    const h = await start({ sessions: [{ ...LESSON, stalled }] });
    const root = await open(h);
    const note = $(root, ".learn-callout[data-kind=budget]");
    expect(note.textContent).toContain("Today's teaching budget is spent");
    expect(byText(note, "a", "Open settings").getAttribute("href")).toContain("settings");
    expect(note.querySelector("button")).toBeNull();
  });

  test("a persisted stall can resume after reopening when the daemon supports retry", async () => {
    const stalled = { reason: "budget_exhausted" as const, message: "today's Noesis interactive budget is spent", at: LESSON.updated_at };
    const h = await start({ sessions: [{ ...LESSON, stalled }], retryTurn: true });
    const root = await open(h);
    await click(h, byText($(root, ".learn-callout[data-kind=budget]"), "button", "Try again"));
    await h.settle(SETTLE_MS);
    expect(h.daemon.sent.filter((command) => command.cmd === "noesis_retry_turn")).toMatchObject([{ session: LESSON.id, idempotency_key: expect.any(String) }]);
    expect(root.querySelector(".learn-callout[data-kind=budget]")).toBeNull();
    expect(root.querySelector(".learn-working")).not.toBeNull();
  });

  test("a stall takes precedence over a previously refused message", async () => {
    const h = await start({ retryTurn: true });
    const root = await open(h);
    h.daemon.refuseTurns = "the session is stalled; retry its turn first";
    await type(h, $(root, "[data-testid=learn-reply]"), "Go on");
    await click(h, byText(root, "button", "Send"));
    h.daemon.stall(LESSON.id, { reason: "budget_exhausted", message: "today's Noesis interactive budget is spent", at: "2026-01-01T00:00:09Z" });
    await h.settle(SETTLE_MS);
    expect(($(root, "[data-testid=learn-reply]") as HTMLTextAreaElement).disabled).toBe(true);
    h.daemon.refuseTurns = undefined;
    await click(h, byText($(root, ".learn-callout[data-kind=budget]"), "button", "Try again"));
    await h.settle(SETTLE_MS);
    expect(h.daemon.sent.filter((command) => command.cmd === "noesis_retry_turn")).toHaveLength(1);
    expect(h.daemon.sent.filter((command) => command.cmd === "noesis_send")).toHaveLength(1);
  });
});

describe("the end of the path", () => {
  test("a plan with every step done shows a completion card whose Practise starts a practice session", async () => {
    const done = { ...PLAN, nodes: PLAN.nodes.map((node) => ({ ...node, status: "visited" as const })) };
    const h = await start({ sessions: [{ ...LESSON, plan: done }] });
    const root = await open(h);
    const card = $(root, "[aria-label='Lesson complete']");
    expect(card.textContent).toContain(`all ${PLAN.nodes.length} steps`);
    expect(root.querySelector(".learn-plan")?.textContent).toContain(`${PLAN.nodes.length} of ${PLAN.nodes.length} done`);
    await click(h, byText(card, "button", "Practise"));
    expect(h.daemon.sent.filter((c) => c.cmd === "noesis_create_session")).toMatchObject([{ goal: LESSON.goal, mode: "practice" }]);
    expect(location.hash).toMatch(/^#\/learn\/sessions\/ns_faux_/);
  });

  test("a plan with a step left shows no completion card, and its count agrees with the steps", async () => {
    const h = await start();
    const root = await open(h);
    expect(root.querySelector("[aria-label='Lesson complete']")).toBeNull();
    const visited = PLAN.nodes.filter((node) => node.status === "visited").length;
    expect(root.querySelector(".learn-plan")?.textContent).toContain(`${visited} of ${PLAN.nodes.length} done`);
  });
});
