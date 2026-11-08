// @vitest-environment happy-dom
// Reading the transcript: tool rows that fold, find, the connection banner and who speaks.

import type { aether } from "@aether-ui/protocol";
import { act, useState } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { Conversation } from "./Conversation.tsx";
import { connected, type Harness } from "./fixture/harness.test-util.tsx";
import { FAUX_AGENT, FAUX_PARENT, fauxAgent, text } from "./fixture/transcript.ts";
import { ToolActivityContext, ToolCall } from "./ToolCall.tsx";
import { ToggleContext, ToggleSet } from "./toggles.ts";

let current: Harness | null = null;

afterEach(() => {
  current?.unmount();
  current = null;
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

/** A conversation whose layout a button switches, as the chat's mode bar does. */
function Layouts({ conn }: { readonly conn: string }) {
  const [layout, setLayout] = useState("standard");
  return <>
    <button type="button" data-testid="layout-focus" onClick={() => setLayout("focus+details")}>Focus</button>
    <Conversation conn={conn} agent={FAUX_AGENT} layout={layout} />
  </>;
}

describe("a tool call", () => {
  const seed: Seed = [
    { message: { role: "assistant", content: [{ type: "tool_use", id: "toolu_1", name: "bash", input: { command: "ls -la crates" } }] }, invocation: "inv_1" },
    { message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu_1", content: [{ type: "text", text: "aether-daemon\naether-cli" }], is_error: false }] } },
  ];

  test("shows an interrupted call even before a result arrives", async () => {
    const h = current = await connected({ instance: "faux-1", agents: [fauxAgent()] });
    const activity = new Map([["toolu_1", { state: "interrupted" as const, durationMs: null }]]);
    const part = { kind: "tool" as const, id: "toolu_1", name: "bash", input: "{}", result: null };
    const container = await h.mount(<ToggleContext.Provider value={new ToggleSet()}><ToolActivityContext.Provider value={activity}><ToolCall part={part} live={false} toggleKey="t:interrupted" /></ToolActivityContext.Provider></ToggleContext.Provider>);
    expect(container.querySelector(".conv-tool")?.getAttribute("data-state")).toBe("interrupted");
    expect(container.textContent).toContain("Interrupted");
    expect(container.textContent).not.toContain("No result yet");
  });

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
    expect(container.querySelector(".conv-count")?.textContent).toBe("1 messages");
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

  test("finds the same term again after Find was closed: the match scrolls into view afresh", async () => {
    const { h, container } = await open([{ message: text("user", "A haystack.") }, { message: text("assistant", "Nothing else.") }]);
    const scrolls = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { value: scrolls, configurable: true, writable: true });
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    const find = async () => {
      await act(async () => container.querySelector<HTMLButtonElement>(".conv-find")?.click());
      const input = byTestId<HTMLInputElement>(container, "conv-search");
      if (!input) throw new Error("Find opens its field");
      await act(async () => {
        setter?.call(input, "haystack");
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
      await h.settle();
      expect(byTestId(container, "conv-search-count")?.textContent).toBe("1 of 1");
      return input;
    };
    const input = await find();
    expect(scrolls).toHaveBeenCalledTimes(1);
    await act(async () => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
    expect(byTestId(container, "conv-search")).toBeNull();
    await find();
    expect(scrolls, "the second find scrolls to the match again").toHaveBeenCalledTimes(2);
  });

  test("keeps a match inside a fold open when the layout changes its row keys", async () => {
    const h = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
    current = h;
    h.daemon.seed(FAUX_AGENT, [{ message: { role: "assistant", content: [{ type: "thinking", text: "the needle hides in thought" }, { type: "text", text: "Looking." }] }, invocation: "inv_1" }]);
    const container = await h.mount(<Layouts conn={h.conn} />);
    await h.settle();
    const fold = () => container.querySelector<HTMLButtonElement>(".conv-reasoning .conv-fold-toggle");
    expect(fold()?.getAttribute("aria-expanded"), "reasoning starts folded").toBe("false");
    await act(async () => container.querySelector<HTMLButtonElement>(".conv-find")?.click());
    const input = byTestId<HTMLInputElement>(container, "conv-search");
    if (!input) throw new Error("Find opens its field");
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    await act(async () => {
      setter?.call(input, "needle");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await h.settle();
    expect(byTestId(container, "conv-search-count")?.textContent).toBe("1 of 1");
    expect(fold()?.getAttribute("aria-expanded"), "the match's fold opens").toBe("true");
    await act(async () => byTestId<HTMLButtonElement>(container, "layout-focus")?.click());
    await h.settle();
    expect(byTestId(container, "conv-search-count")?.textContent).toBe("1 of 1");
    expect(fold()?.getAttribute("aria-expanded"), "the fold under its new key opens too").toBe("true");
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
    expect(container.textContent).toContain("Write to it in the box below");
  });

  test("shows the disconnected banner even when the transcript is empty", async () => {
    const { h, container } = await open([]);
    expect(byTestId(container, "conv-connection")).toBeNull();
    h.daemon.drop(1006);
    await h.settle();
    expect(byTestId(container, "conv-connection")?.textContent).toMatch(/lost|Disconnected/);
    expect(container.textContent).toContain("No messages yet");
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
