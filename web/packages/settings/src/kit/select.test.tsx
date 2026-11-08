// @vitest-environment happy-dom
// The confirm step's reason, a TextArea, reaches the command trimmed.

import { act } from "react";
import { afterEach, describe, expect, test } from "vitest";
import { buttonNamed, click, connected, type Harness } from "./fixture/harness.test-util.tsx";
import { Action } from "./kit.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

async function typeInto(h: Harness, element: Element, value: string): Promise<void> {
  expect(element).toBeInstanceOf(HTMLTextAreaElement);
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await h.settle();
}

describe("the confirm step's reason", () => {
  test("keeps confirm off until a required reason is typed, then sends it trimmed", async () => {
    const h = current = await connected({ role: "local" });
    const reasons: string[] = [];
    const root = await h.mount(<Action conn={h.conn} command={{ cmd: "cancel", agent: "agt_1" }} label="Go" onRun={(reason) => { reasons.push(reason); }}
      confirm={{ title: "Sure?", body: "It cannot be undone.", confirmLabel: "Yes, go", reason: "required" }} />);
    await click(h, root, "Go");
    const field = document.querySelector("[data-testid=settings-confirm-reason]");
    expect(buttonNamed(document.body, "Yes, go").disabled).toBe(true);
    await typeInto(h, field as Element, "  stuck on a lock  ");
    expect(buttonNamed(document.body, "Yes, go").disabled).toBe(false);
    await click(h, document.body, "Yes, go");
    expect(reasons).toEqual(["stuck on a lock"]);
  });
});
