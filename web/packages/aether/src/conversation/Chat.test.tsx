// @vitest-environment happy-dom
// The chat's view modes end to end in one screen: the catalog read from the daemon, the default and
// remembered choice, the picker, `/mode` commands, Details, and decisions pinned until answered.

import type { aether } from "@aether-ui/protocol";
import { act } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { controlsScript } from "../controls/fixture/demo.ts";
import type { Answer, ScriptOptions } from "../controls/fixture/daemon.ts";
import { buttonNamed, click, connected, type Harness } from "../controls/fixture/harness.test-util.tsx";
import { invariant } from "../fleet/invariant.ts";
import { Chat } from "./Chat.tsx";
import { fauxAgent } from "./fixture/transcript.ts";
import { modeKey } from "./useViewMode.ts";
import { firstMessageKey } from "../activity/firstMessage.ts";
import { forgetDrafts } from "../composer/drafts.ts";

const CALM: aether.ViewMode = {
  name: "calm", builtin: false, label: "Calm", description: "Archon coordination", density: "compact",
  show: ["user_text", "assistant_text", "delegation", "agent_message", "completion", "status", "question", "approval", "error"],
  scope: { role: "archon-orchestrator" },
};
const FOCUS: aether.ViewMode = { ...CALM, name: "focus", builtin: true, label: "Focus", scope: null };
const STANDARD: aether.ViewMode = { name: "standard", builtin: true, label: "Standard", density: "comfortable", show: [...CALM.show, "tool", "reasoning", "memory", "compaction", "usage"] };

const orchestrator = fauxAgent("agt_01JFAUX00000000000000000AR", { name: "Archon", role: "archon-orchestrator", parent: null, status: { state: "waiting", condition: { on: "inbox" } } });
const ENTRIES: aether.TranscriptEntry[] = [
  { seq: 1, at: "2026-11-07T09:00:00Z", invocation: null, message: { role: "user", content: [{ type: "text", text: "What is running?" }] } },
  { seq: 2, at: "2026-11-07T09:00:01Z", invocation: "inv_1", message: { role: "assistant", content: [{ type: "text", text: "Checking the queue." }, { type: "tool_use", id: "t1", name: "bash", input: { command: "archon queue" } }] } },
  { seq: 3, at: "2026-11-07T09:00:02Z", invocation: null, message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: [{ type: "text", text: "3 running" }] }] } },
  { seq: 4, at: "2026-11-07T09:00:03Z", invocation: "inv_2", message: { role: "assistant", content: [{ type: "text", text: "Three items are running." }] } },
];
const page = (): Answer => ({ tag: "transcript_page", data: { entries: ENTRIES, has_before: false, has_after: false, last_seq: 4 } });
const catalog = (): Answer => ({ tag: "view_modes", data: { modes: [STANDARD, FOCUS, CALM], diagnostics: [] } });

let current: Harness | null = null;
afterEach(() => {
  current?.unmount();
  current = null;
  localStorage.clear();
  sessionStorage.clear();
  forgetDrafts();
});

async function open(agent: aether.Agent = orchestrator, options: ScriptOptions = {}) {
  const h = current = await connected({ agents: [agent], ...options, script: { ...controlsScript(), get_transcript_page: page, list_view_modes: catalog, ...options.script } });
  const root = await h.mount(<Chat conn={h.conn} agent={agent} />);
  return { h, root };
}

const checked = (root: ParentNode) => root.querySelector('[role="radio"][aria-checked="true"]')?.textContent;
const radios = (root: ParentNode) => [...root.querySelectorAll('[role="radio"]')].map((r) => r.textContent);
const toolShown = (root: ParentNode) => root.querySelector(".conv-tool") !== null;
const textarea = (root: ParentNode) => {
  const input = root.querySelector("textarea");
  invariant(input instanceof HTMLTextAreaElement, "the chat has a composer");
  return input;
};

async function write(h: Harness, input: HTMLTextAreaElement, value: string): Promise<void> {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  invariant(setter !== undefined, "a textarea has a value setter");
  await act(async () => {
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await h.settle();
}

async function key(h: Harness, target: Element, init: KeyboardEventInit): Promise<void> {
  await act(async () => { target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init })); });
  await h.settle();
}

describe("Chat view modes", () => {
  test("Archon's orchestrator opens in its scoped Calm mode, which hides tool calls", async () => {
    const { root } = await open();
    expect(radios(root)).toEqual(["Standard", "Focus", "Calm"]);
    expect(checked(root)).toBe("Calm");
    expect(toolShown(root)).toBe(false);
    expect(root.querySelector(".conv-chat")?.getAttribute("data-mode")).toBe("calm");
    expect(root.textContent).toContain("Three items are running.");
  });

  test("another agent opens in standard and is offered no mode scoped elsewhere", async () => {
    const worker = fauxAgent(undefined, { parent: null });
    const { root } = await open(worker);
    expect(radios(root)).toEqual(["Standard", "Focus"]);
    expect(checked(root)).toBe("Standard");
    expect(toolShown(root)).toBe(true);
  });

  test("the picker's choice is remembered for that agent and keeps the draft", async () => {
    const { h, root } = await open();
    await write(h, textarea(root), "half a thought");
    await click(h, root, "Standard");
    expect(checked(root)).toBe("Standard");
    expect(toolShown(root)).toBe(true);
    expect(textarea(root).value).toBe("half a thought");
    expect(localStorage.getItem(modeKey(h.conn, orchestrator.id))).toBe("standard");
    current?.unmount();
    const again = await open();
    expect(checked(again.root), "a new screen opens in the saved mode").toBe("Standard");
  });

  test("arrow keys move the choice within the picker", async () => {
    const { h, root } = await open();
    const group = root.querySelector('[role="radiogroup"]');
    invariant(group !== null, "the picker is a radio group");
    await key(h, group, { key: "ArrowRight" });
    expect(checked(root), "right of the last wraps to the first").toBe("Standard");
    await key(h, group, { key: "End" });
    expect(checked(root)).toBe("Calm");
  });

  test("/focus and /mode run from the composer and leave no draft behind", async () => {
    const { h, root } = await open();
    const input = textarea(root);
    await write(h, input, "/focus");
    await key(h, input, { key: "Escape" });
    await key(h, input, { key: "Enter" });
    expect(checked(root)).toBe("Focus");
    expect(input.value).toBe("");
    await write(h, input, "/focus");
    await key(h, input, { key: "Escape" });
    await key(h, input, { key: "Enter" });
    expect(checked(root), "the same toggle goes back to standard").toBe("Standard");
    await write(h, input, "/mode calm");
    await key(h, input, { key: "Escape" });
    await key(h, input, { key: "Enter" });
    expect(checked(root)).toBe("Calm");
    expect(h.daemon.commands("send_message"), "view commands never reach the agent").toEqual([]);
  });

  test("Details, by button or Ctrl+T, reveals what the mode hides without leaving it", async () => {
    const { h, root } = await open();
    await click(h, root, /Details/);
    expect(buttonNamed(root, /Details/).getAttribute("aria-pressed")).toBe("true");
    expect(toolShown(root)).toBe(true);
    expect(checked(root)).toBe("Calm");
    const chat = root.querySelector(".conv-chat");
    invariant(chat !== null, "the chat is mounted");
    await key(h, chat, { key: "t", ctrlKey: true });
    expect(toolShown(root)).toBe(false);
  });

  test("an open question for this agent is pinned above the transcript, others are not", async () => {
    const question = (agent: string, seq: number): aether.PendingQuestion => ({ agent, question: `Merge ${seq}?`, question_seq: seq, asked_at: "2026-11-07T09:00:00Z" });
    const { root } = await open(orchestrator, { attention: { questions: [question(orchestrator.id, 1), question("agt_other", 2)] } });
    const pinned = root.querySelector('[aria-label="Waiting for you"]');
    expect(pinned?.textContent).toContain("Merge 1?");
    expect(pinned?.textContent).not.toContain("Merge 2?");
    expect(root.querySelector(".conv-activity")?.textContent).toBe("Waiting on inbox");
  });

  test("a failed edit in Calm is one named line, never hidden with the other tool detail", async () => {
    const failed: aether.TranscriptEntry[] = [
      ...ENTRIES,
      { seq: 5, at: "2026-11-07T09:00:04Z", invocation: "inv_3", message: { role: "assistant", content: [{ type: "tool_use", id: "t2", name: "edit_file", input: { path: "a.rs" } }] } },
      { seq: 6, at: "2026-11-07T09:00:05Z", invocation: null, message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t2", content: [{ type: "text", text: "no match for the old text\nat line 4" }], is_error: true }] } },
    ];
    const withEdit = (): Answer => ({ tag: "transcript_page", data: { entries: failed, has_before: false, has_after: false, last_seq: 6 } });
    const { root } = await open(orchestrator, { script: { get_transcript_page: withEdit } });
    expect(checked(root)).toBe("Calm");
    expect(toolShown(root)).toBe(false);
    expect([...root.querySelectorAll(".conv-line")].map((l) => l.textContent)).toEqual(["!edit_file failedno match for the old text"]);
  });

  test("a daemon without view modes keeps the standard view and says why", async () => {
    const refuse = (): Answer => ({ code: "invalid_argument", message: "unknown command list_view_modes" });
    const { root } = await open(orchestrator, { script: { list_view_modes: refuse } });
    expect(radios(root)).toEqual([]);
    expect(toolShown(root)).toBe(true);
    expect(root.textContent).toContain("View modes unavailable");
  });
});

describe("Chat after its start", () => {
  test("a first message the start could not deliver is shown in the chat and sent from it as typed", async () => {
    const agent = fauxAgent(undefined, { status: { state: "waiting", condition: { on: "inbox" } } });
    sessionStorage.setItem(firstMessageKey("aether:b3", agent.id), JSON.stringify({ text: "/calm please", outcome: "unconfirmed", reason: "No reply" }));
    const { h, root } = await open(agent);
    const notice = root.querySelector('[aria-label="First message not delivered"]');
    expect(notice?.textContent).toContain("it may have arrived");
    expect(notice?.querySelector('[aria-label="First message, as typed"]')?.textContent).toBe("/calm please");
    await click(h, root, "Send it as typed");
    expect(h.daemon.commands("send_message").map((c) => c.content), "sent to the agent, not run as /calm").toEqual(["/calm please"]);
    expect(root.querySelector('[aria-label="First message not delivered"]')).toBeNull();
    expect(checked(root), "the view mode is untouched").toBe("Standard");
  });

  test("a stored record that is not a whole first message is dropped, not shown", async () => {
    const agent = fauxAgent(undefined, { status: { state: "waiting", condition: { on: "inbox" } } });
    const key = firstMessageKey("aether:b3", agent.id);
    sessionStorage.setItem(key, JSON.stringify({ text: " padded ", outcome: "refused", reason: "x" }));
    const { root } = await open(agent);
    expect(root.querySelector('[aria-label="First message not delivered"]')).toBeNull();
    expect(sessionStorage.getItem(key)).toBeNull();
  });

  test("a store that cannot be read shows no first message and leaves the chat working", async () => {
    const agent = fauxAgent(undefined, { status: { state: "waiting", condition: { on: "inbox" } } });
    const getItem = vi.spyOn(sessionStorage, "getItem").mockImplementation(() => { throw new DOMException("denied", "SecurityError"); });
    try {
      const { root } = await open(agent);
      expect(root.querySelector('[aria-label="First message not delivered"]')).toBeNull();
      expect(root.querySelector("textarea"), "the composer is there").not.toBeNull();
      expect(root.textContent, "its draft is said to live only until a reload").toContain("not across a reload");
    } finally {
      getItem.mockRestore();
    }
  });
});
