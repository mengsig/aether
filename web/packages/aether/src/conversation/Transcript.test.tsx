// @vitest-environment happy-dom
// Reading the transcript: tool rows that fold, the jump back to the newest message, and find.

import { drafts } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { act } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { composerKey } from "./Composer.tsx";
import { Conversation } from "./Conversation.tsx";
import { connected, type Harness } from "./fixture/harness.test-util.tsx";
import { FAUX_AGENT, FAUX_PARENT, fauxAgent, text } from "./fixture/transcript.ts";
import { forgetScrollPositions } from "./TranscriptList.tsx";

let current: Harness | null = null;

afterEach(() => {
  if (current) drafts.clear(composerKey(current.conn, FAUX_AGENT));
  current?.unmount();
  current = null;
  forgetScrollPositions();
  vi.restoreAllMocks();
});

type Seed = Parameters<Harness["daemon"]["seed"]>[1];

async function open(seed: Seed) {
  const h = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
  current = h;
  h.daemon.seed(FAUX_AGENT, seed);
  const container = await h.mount(<Conversation conn={h.conn} agent={FAUX_AGENT} />);
  await h.settle();
  return { h, container };
}

const byTestId = <T extends HTMLElement = HTMLElement>(root: ParentNode, id: string) => root.querySelector<T>(`[data-testid=${id}]`);

describe("a tool call", () => {
  const seed: Seed = [
    { message: { role: "assistant", content: [{ type: "tool_use", id: "toolu_1", name: "bash", input: { command: "ls -la crates" } }] }, invocation: "inv_1" },
    { message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu_1", content: [{ type: "text", text: "aether-daemon\naether-cli" }], is_error: false }] } },
  ];

  test("is one folded row with its name and argument, and opens and closes its output", async () => {
    const { container } = await open(seed);
    const toggle = byTestId<HTMLButtonElement>(container, "conv-tool-toggle");
    const output = byTestId(container, "conv-tool-output");
    if (!toggle || !output) throw new Error("the tool row and its output are drawn");
    expect(toggle.textContent).toContain("bash");
    expect(toggle.textContent).toContain("ls -la crates");
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(output.hidden).toBe(true);
    expect(toggle.getAttribute("aria-controls")).toBe(output.id);
    await act(async () => toggle.click());
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(output.hidden).toBe(false);
    expect(output.textContent).toContain("aether-cli");
    expect(output.querySelector(".conv-copy")).not.toBeNull();
    await act(async () => toggle.click());
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(output.hidden).toBe(true);
  });

  test("is counted in the header", async () => {
    const { container } = await open(seed);
    expect(container.querySelector(".conv-count")?.textContent).toBe("1 message · 1 tool call");
  });
});

/** A viewport 400 px high over a long list, so the list can be scrolled away from its end. */
function fakeGeometry(): void {
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockImplementation(function (this: HTMLElement) {
    return this.classList.contains("conv-list") ? 400 : 0;
  });
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(function (this: HTMLElement) {
    return this.classList.contains("conv-list") ? Number.parseFloat(this.querySelector<HTMLElement>(".conv-spacer")?.style.blockSize ?? "0") : 0;
  });
}

const many = (count: number): Seed => Array.from({ length: count }, (_, i) => ({ message: text(i % 2 === 0 ? "user" : "assistant", `Message ${i}`) }));

describe("jump to latest", () => {
  test("appears once the reader scrolls up, counts what arrives, and goes back to the end", async () => {
    fakeGeometry();
    const { h, container } = await open(many(60));
    const list = container.querySelector<HTMLElement>(".conv-list");
    if (!list) throw new Error("the list is drawn");
    expect(byTestId(container, "conv-jump-latest")).toBeNull();
    await act(async () => {
      list.scrollTop = 0;
      list.dispatchEvent(new Event("scroll"));
    });
    expect(byTestId(container, "conv-jump-latest")?.textContent).toBe("↓Jump to latest");
    h.daemon.commit(FAUX_AGENT, text("assistant", "Something new"));
    h.daemon.commit(FAUX_AGENT, text("assistant", "And more"));
    await h.settle();
    const jump = byTestId<HTMLButtonElement>(container, "conv-jump-latest");
    expect(jump?.textContent).toContain("2 new");
    expect(container.querySelector("[role=status][aria-live=polite]")?.textContent).toBe("2 new messages below");
    expect(list.scrollTop).toBe(0);
    await act(async () => jump?.click());
    await h.settle();
    expect(byTestId(container, "conv-jump-latest")).toBeNull();
    expect(list.scrollTop + 400).toBe(list.scrollHeight);
  });
});

describe("the reading position", () => {
  test("is where the reader left this agent's transcript, when they come back", async () => {
    fakeGeometry();
    const { h, container } = await open(many(60));
    const list = container.querySelector<HTMLElement>(".conv-list");
    await act(async () => {
      if (list) list.scrollTop = 300;
      list?.dispatchEvent(new Event("scroll"));
    });
    h.unmount();
    const again = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
    current = again;
    again.daemon.seed(FAUX_AGENT, many(60));
    const back = await again.mount(<Conversation conn={again.conn} agent={FAUX_AGENT} />);
    await again.settle();
    expect(back.querySelector<HTMLElement>(".conv-list")?.scrollTop).toBe(300);
    expect(byTestId(back, "conv-jump-latest")).not.toBeNull();
  });
});

describe("find", () => {
  test("opens with Ctrl+F in the transcript, counts the matches and steps through them", async () => {
    const { h, container } = await open([
      { message: text("user", "Where is the needle?") },
      { message: text("assistant", "No needle here, but a haystack.") },
      { message: text("user", "Then find the needle and the other needle.") },
    ]);
    const stage = container.querySelector<HTMLElement>(".conv-stage");
    const keydown = new KeyboardEvent("keydown", { key: "f", ctrlKey: true, bubbles: true, cancelable: true });
    await act(async () => stage?.dispatchEvent(keydown));
    expect(keydown.defaultPrevented).toBe(true);
    const input = byTestId<HTMLInputElement>(container, "conv-search");
    if (!input) throw new Error("Ctrl+F opens the find field");
    expect(document.activeElement).toBe(input);
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    await act(async () => {
      setter?.call(input, "NEEDLE");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await h.settle();
    const count = () => byTestId(container, "conv-search-count")?.textContent;
    const currentSeq = () => container.querySelector(".conv-msg[data-current=true]")?.getAttribute("data-seq");
    // A new search lands on the newest match.
    expect(count()).toBe("4 of 4");
    expect(currentSeq()).toBe("3");
    await act(async () => byTestId(container, "conv-search-next")?.click());
    expect(count()).toBe("1 of 4");
    expect(currentSeq()).toBe("1");
    await act(async () => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })));
    expect(count()).toBe("2 of 4");
    expect(currentSeq()).toBe("2");
    await act(async () => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", shiftKey: true, bubbles: true, cancelable: true })));
    await act(async () => byTestId(container, "conv-search-prev")?.click());
    expect(count()).toBe("4 of 4");
    await act(async () => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
    expect(byTestId(container, "conv-search")).toBeNull();
    expect(container.querySelector(".conv-msg[data-current=true]")).toBeNull();
  });

  test("says when nothing matches", async () => {
    const { h, container } = await open([{ message: text("user", "Hello") }]);
    await act(async () => container.querySelector<HTMLButtonElement>(".conv-find")?.click());
    const input = byTestId<HTMLInputElement>(container, "conv-search");
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    await act(async () => {
      setter?.call(input, "absent");
      input?.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await h.settle();
    expect(byTestId(container, "conv-search-count")?.textContent).toBe("No matches");
    expect(byTestId<HTMLButtonElement>(container, "conv-search-next")?.disabled).toBe(true);
  });
});

describe("the screen's states", () => {
  test("points at the composer when there are no messages yet", async () => {
    const { container } = await open([]);
    expect(container.textContent).toContain("No messages yet");
    expect(byTestId(container, "conv-composer")).not.toBeNull();
  });

  test("keeps the transcript and shows a quiet banner while the connection is down", async () => {
    const { h, container } = await open([{ message: text("user", "Still here") }]);
    expect(byTestId(container, "conv-connection")).toBeNull();
    h.daemon.drop(1006);
    await h.settle();
    expect(byTestId(container, "conv-connection")?.textContent).toMatch(/lost|Disconnected/);
    expect(container.querySelector(".conv-msg-body")?.textContent).toBe("Still here");
  });
});

describe("who speaks", () => {
  const inbox = (header: string, body: string): aether.Message => text("user", `${header}\n${body}`);

  test("names the operator, another agent and a daemon notice apart", async () => {
    const { container } = await open([
      { message: text("user", "Plain prompt") },
      { message: inbox("[inbox 1] Message from agt_01JFAUX0000000000000000000:", "Check the paging.") },
      { message: text("user", "[inbox 2] Task tsk_1 completed.") },
    ]);
    const speakers = [...container.querySelectorAll(".conv-msg")].map((node) => [node.getAttribute("data-speaker"), node.querySelector(".conv-msg-body")?.textContent]);
    expect(speakers).toEqual([
      ["user", "Plain prompt"],
      ["agent", "Check the paging."],
      ["notice", "[inbox 2] Task tsk_1 completed."],
    ]);
  });
});
