// @vitest-environment happy-dom
// The composer over a faux daemon: what it sends, when it sends, and what becomes of the draft.

import { drafts } from "@aether-ui/core";
import { act } from "react";
import { afterEach, describe, expect, test } from "vitest";
import { Composer, composerKey } from "./Composer.tsx";
import { connected, type Harness } from "./fixture/harness.test-util.tsx";
import { FAUX_AGENT, FAUX_PARENT, fauxAgent } from "./fixture/transcript.ts";

let current: Harness | null = null;

afterEach(() => {
  if (current) drafts.clear(composerKey(current.conn, FAUX_AGENT));
  current?.unmount();
  current = null;
});

async function open(agent = fauxAgent(), id = FAUX_AGENT) {
  const h = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), agent] });
  current = h;
  const container = await h.mount(<Composer conn={h.conn} agent={id} />);
  return { h, container };
}

function parts(container: HTMLElement) {
  const input = container.querySelector<HTMLTextAreaElement>("[data-testid=composer-input]");
  const send = container.querySelector<HTMLButtonElement>("[data-testid=composer-send]");
  if (!input || !send) throw new Error("the composer draws its input and send button");
  return { input, send };
}

async function type(h: Harness, input: HTMLTextAreaElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  if (!setter) throw new Error("a text area has a value setter");
  await act(async () => {
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await h.settle();
}

async function press(h: Harness, input: HTMLTextAreaElement, init: KeyboardEventInit) {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  await act(async () => input.dispatchEvent(event));
  await h.settle();
  return event;
}

/** The send_message commands the daemon received, without the transport's own fields. */
const sentMessages = (h: Harness) => h.daemon.commands("send_message").map((command) =>
  Object.fromEntries(Object.entries(command).filter(([key]) => key !== "req" && key !== "idempotency_key")));

describe("the composer", () => {
  test("sends with Enter the same send_message the Steer control sends, then clears the draft", async () => {
    const { h, container } = await open();
    const { input, send } = parts(container);
    expect(send.disabled).toBe(true);
    await type(h, input, "  Ship it.  ");
    expect(send.disabled).toBe(false);
    const event = await press(h, input, { key: "Enter" });
    expect(event.defaultPrevented).toBe(true);
    expect(sentMessages(h)).toEqual([{ cmd: "send_message", to: FAUX_AGENT, content: "Ship it." }]);
    expect(parts(container).input.value).toBe("");
    expect(drafts.get(composerKey(h.conn, FAUX_AGENT))).toBeUndefined();
    expect(container.textContent).toContain("Sent.");
  });

  test("adds a line with Shift+Enter and sends nothing", async () => {
    const { h, container } = await open();
    const { input } = parts(container);
    await type(h, input, "First line");
    const event = await press(h, input, { key: "Enter", shiftKey: true });
    expect(event.defaultPrevented).toBe(false);
    expect(sentMessages(h)).toEqual([]);
    expect(parts(container).input.value).toBe("First line");
  });

  test("asks for subtree only when the switch is on and the agent has delegates", async () => {
    const { h, container } = await open(fauxAgent(FAUX_AGENT), FAUX_PARENT);
    const toggle = container.querySelector<HTMLElement>("[role=switch]");
    if (!toggle) throw new Error("the parent's composer offers the subtree switch");
    await act(async () => toggle.click());
    const { input, send } = parts(container);
    await type(h, input, "Everyone, stop.");
    await act(async () => send.click());
    await h.settle();
    expect(sentMessages(h)).toEqual([{ cmd: "send_message", to: FAUX_PARENT, content: "Everyone, stop.", subtree: true }]);
  });

  test("keeps the draft across leaving the screen and coming back", async () => {
    const { h, container } = await open();
    await type(h, parts(container).input, "Half a thought");
    h.unmount();
    const again = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
    current = again;
    const remounted = await again.mount(<Composer conn={again.conn} agent={FAUX_AGENT} />);
    expect(parts(remounted).input.value).toBe("Half a thought");
  });

  test("keeps the draft when the send fails, says why inline and retries", async () => {
    const { h, container } = await open();
    h.daemon.refuse.set("send_message", { code: "invalid_argument", message: "the agent is gone" });
    await type(h, parts(container).input, "Are you there?");
    await act(async () => parts(container).send.click());
    await h.settle();
    expect(container.querySelector("[role=alert]")?.textContent).toContain("the agent is gone");
    expect(parts(container).input.value).toBe("Are you there?");
    h.daemon.refuse.clear();
    const retry = container.querySelector<HTMLButtonElement>("[data-testid=composer-retry]");
    await act(async () => retry?.click());
    await h.settle();
    expect(sentMessages(h)).toHaveLength(2);
    expect(parts(container).input.value).toBe("");
  });

  test("is off, with the reason, once the agent has finished", async () => {
    const { h, container } = await open(fauxAgent(FAUX_AGENT, { status: { state: "completed" } }));
    const { input, send } = parts(container);
    expect(input.disabled).toBe(true);
    expect(send.disabled).toBe(true);
    expect(container.querySelector(".conv-composer-note[data-state=blocked]")?.textContent).toContain("builder has completed");
    await press(h, input, { key: "Enter" });
    expect(sentMessages(h)).toEqual([]);
  });
});
