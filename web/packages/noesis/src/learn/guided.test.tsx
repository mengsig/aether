// @vitest-environment happy-dom
// The guided learning fields: goal, reply and answer drafts that survive leaving the screen and clear on
// success, Enter against Shift+Enter in the composer, Segmented choices sending the values the old
// controls sent, and the empty graph's way out.

import { drafts } from "@aether-ui/core";
import { act } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { sendsOnKey } from "./Composer.tsx";
import { SETTLE_MS } from "./data.ts";
import { $, byText, click, CONN, connected, stubCanvas, type, type Harness } from "./fixture/harness.test-util.tsx";
import { EARLIER, LESSON, PLAN, PRACTICE, RECALL_QUESTION, scenario } from "./fixture/scenario.ts";
import { GraphScreen } from "./GraphScreen.tsx";
import { LearnFrame } from "./LearnFrame.tsx";
import { draftKey, planProgress, recentGoals } from "./model.ts";
import SessionRoute, { scrolledAway } from "./SessionScreen.tsx";
import SessionsRoute from "./SessionsScreen.tsx";

let harness: Harness | null = null;
afterEach(() => {
  harness?.unmount();
  harness = null;
});

const REPLY_DRAFT = draftKey(LESSON.id, "reply");
const sent = (h: Harness, cmd: string) => h.daemon.sent.filter((command) => command.cmd === cmd);

async function openLesson(): Promise<{ h: Harness; root: HTMLElement }> {
  const h = await connected(scenario(true, { transport: "ipc" }), vi);
  harness = h;
  location.hash = `#/learn/sessions/${LESSON.id}`;
  const root = await h.mount(<SessionRoute params={{ id: LESSON.id }} />);
  await h.settle(SETTLE_MS);
  return { h, root };
}

async function press(h: Harness, element: Element, shiftKey: boolean): Promise<void> {
  await act(async () => { element.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", shiftKey, bubbles: true, cancelable: true })); });
  await h.settle(SETTLE_MS);
}

describe("the new-session form", () => {
  test("keeps a typed goal across leaving the screen and clears it once the session starts", async () => {
    const h = harness = await connected(scenario(true), vi);
    let root = await h.mount(<SessionsRoute />);
    expect(document.activeElement).toBe($(root, "[data-testid=learn-goal]"));
    await type(h, $(root, "[data-testid=learn-goal]"), "How TCP slow start works");
    root = await h.remount(<SessionsRoute />);
    expect(($(root, "[data-testid=learn-goal]") as HTMLTextAreaElement).value).toBe("How TCP slow start works");
    await click(h, $(root, "[data-testid=learn-start]"));
    expect(sent(h, "noesis_create_session")).toMatchObject([{ goal: "How TCP slow start works", mode: "teach" }]);
    expect(drafts.get(draftKey("new-session", CONN))).toBeUndefined();
  });

  test("the mode cards send the mode the old buttons sent, and Start says why it is off", async () => {
    const h = harness = await connected(scenario(true), vi);
    const root = await h.mount(<SessionsRoute />);
    const start = $(root, "[data-testid=learn-start]") as HTMLButtonElement;
    expect(start.disabled).toBe(true);
    expect(root.querySelector("#learn-start-why")?.textContent).toBe("Name a goal to start");
    await click(h, byText($(root, "[data-testid=learn-mode]"), "[role=radio]", "Practise"));
    expect(byText(root, "[role=radio]", "Practise").getAttribute("aria-checked")).toBe("true");
    await type(h, $(root, "[data-testid=learn-goal]"), "Split votes");
    expect(start.disabled).toBe(false);
    expect(root.querySelector("#learn-start-why")).toBeNull();
    await click(h, start);
    expect(sent(h, "noesis_create_session")).toMatchObject([{ goal: "Split votes", mode: "practice" }]);
  });

  test("recent goals are offered while the goal is empty, and picking one fills it", async () => {
    const h = harness = await connected(scenario(true), vi);
    const root = await h.mount(<SessionsRoute />);
    await h.settle(SETTLE_MS);
    const chips = [...$(root, "[data-testid=learn-goal-suggestions]").querySelectorAll("button")].map((chip) => chip.textContent);
    expect(chips).toEqual([PRACTICE.goal, LESSON.goal, EARLIER.goal]);
    await click(h, byText(root, ".learn-chip", LESSON.goal));
    expect(($(root, "[data-testid=learn-goal]") as HTMLTextAreaElement).value).toBe(LESSON.goal);
    expect(root.querySelector("[data-testid=learn-goal-suggestions]")).toBeNull();
  });
});

describe("the composer", () => {
  test("Enter sends and clears the reply; Shift+Enter does not send", async () => {
    const { h, root } = await openLesson();
    const reply = $(root, "[data-testid=learn-reply]");
    await type(h, reply, "Why a majority?");
    await press(h, reply, true);
    expect(sent(h, "noesis_send")).toHaveLength(0);
    await press(h, reply, false);
    expect(sent(h, "noesis_send")).toMatchObject([{ session: LESSON.id, text: "Why a majority?" }]);
    expect(($(root, "[data-testid=learn-reply]") as HTMLTextAreaElement).value).toBe("");
    expect(drafts.get(REPLY_DRAFT)).toBeUndefined();
  });

  test("keeps the reply per session across leaving it, and Send clears it once sent", async () => {
    const { h, root } = await openLesson();
    await type(h, $(root, "[data-testid=learn-reply]"), "Half-typed thought");
    expect(drafts.get(REPLY_DRAFT)).toEqual({ text: "Half-typed thought" });
    const again = await h.remount(<SessionRoute params={{ id: LESSON.id }} />);
    await h.settle(SETTLE_MS);
    expect(($(again, "[data-testid=learn-reply]") as HTMLTextAreaElement).value).toBe("Half-typed thought");
    await click(h, $(again, "[data-testid=learn-send]"));
    await h.settle(SETTLE_MS);
    expect(sent(h, "noesis_send")).toMatchObject([{ text: "Half-typed thought" }]);
    expect(drafts.get(REPLY_DRAFT)).toBeUndefined();
  });

  test("only Enter alone sends, and never while an input method is composing", () => {
    expect(sendsOnKey({ key: "Enter", shiftKey: false, altKey: false })).toBe(true);
    expect(sendsOnKey({ key: "Enter", shiftKey: true, altKey: false })).toBe(false);
    expect(sendsOnKey({ key: "Enter", shiftKey: false, altKey: true })).toBe(false);
    expect(sendsOnKey({ key: "a", shiftKey: false, altKey: false })).toBe(false);
    expect(sendsOnKey({ key: "Enter", shiftKey: false, altKey: false, nativeEvent: { isComposing: true } })).toBe(false);
  });
});

describe("a question's answer", () => {
  test("is kept per question across leaving the session and cleared once recorded", async () => {
    const { h, root } = await openLesson();
    const key = draftKey(RECALL_QUESTION.question.id, "answer");
    h.daemon.pose(LESSON.id, RECALL_QUESTION);
    await h.settle(SETTLE_MS);
    await type(h, $(root, "[data-testid=learn-answer]"), "It steps down");
    expect(drafts.get(key)).toMatchObject({ text: "It steps down" });
    const again = await h.remount(<SessionRoute params={{ id: LESSON.id }} />);
    await h.settle(SETTLE_MS);
    const card = $(again, ".learn-q[data-state=open]");
    expect(($(card, "[data-testid=learn-answer]") as HTMLTextAreaElement).value).toBe("It steps down");
    await click(h, byText(card, "button", "Answer"));
    await h.settle(SETTLE_MS);
    expect(sent(h, "noesis_answer").map((command) => command.answer)).toEqual([{ dont_know: false, text: "It steps down", confidence: 50 }]);
    expect(drafts.get(key)).toBeUndefined();
  });
});

describe("the knowledge graph", () => {
  test("an empty graph says how it fills and links to starting a session", async () => {
    stubCanvas();
    const h = harness = await connected(scenario(true, { graph: { nodes: [], chains: [] } }), vi);
    const root = await h.mount(<LearnFrame view="graph">{(gate) => <GraphScreen {...gate} />}</LearnFrame>);
    await h.settle(SETTLE_MS);
    const empty = $(root, "[data-testid=learn-graph-empty]");
    expect(empty.textContent).toContain("Add to knowledge graph");
    expect(empty.querySelector("a")?.getAttribute("href")).toBe("#/learn");
    expect(root.querySelector("canvas")).toBeNull();
  });
});

describe("helpers", () => {
  test("draft keys stay within what a draft key allows", () => {
    expect(draftKey("ns_raft", "reply")).toBe("learn/ns_raft/reply");
    expect(draftKey("understand", "file", "src/a b.ts", "goal")).toBe("learn/understand/file/src-a-b.ts/goal");
    expect(() => draftKey()).toThrow(/names its form/);
  });

  test("plan progress counts done steps, and there is none before a plan", () => {
    const visited = PLAN.nodes.filter((node) => node.status === "visited").length;
    expect(planProgress(LESSON)).toEqual({ done: visited, total: PLAN.nodes.length });
    expect(planProgress(EARLIER)).toBeNull();
  });

  test("recent goals are newest first, without repeats, up to the limit", () => {
    const again = { ...EARLIER, id: "ns_again", goal: ` ${PRACTICE.goal} `, updated_at: "2026-11-01T00:00:00Z" };
    expect(recentGoals([LESSON, PRACTICE, EARLIER, again], 2)).toEqual([PRACTICE.goal, LESSON.goal]);
    expect(recentGoals([])).toEqual([]);
  });

  test("the chat follows only while the reader is near the bottom", () => {
    expect(scrolledAway({ scrollHeight: 1000, scrollTop: 600, clientHeight: 400 })).toBe(false);
    expect(scrolledAway({ scrollHeight: 1000, scrollTop: 560, clientHeight: 400 })).toBe(false);
    expect(scrolledAway({ scrollHeight: 1000, scrollTop: 400, clientHeight: 400 })).toBe(true);
  });
});
