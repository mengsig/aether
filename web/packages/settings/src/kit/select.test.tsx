// @vitest-environment happy-dom
// The kit's guided controls: SelectField drawn by the Picker still hands its caller the option's value,
// and the confirm step's reason, now a TextArea, still reaches the command trimmed.

import { act, useState } from "react";
import { afterEach, describe, expect, test } from "vitest";
import { buttonNamed, click, connected, type Harness } from "./fixture/harness.test-util.tsx";
import { Action } from "./kit.tsx";
import { SelectField } from "./select.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

const DAEMONS = [{ value: "aether:a", label: "Studio" }, { value: "aether:b", label: "Laptop" }];

function Chooser({ picked }: { readonly picked: string[] }) {
  const [value, setValue] = useState("aether:a");
  return <SelectField label="Daemon" value={value} options={DAEMONS} onChange={(next) => { picked.push(next); setValue(next); }} />;
}

async function typeInto(h: Harness, element: Element, value: string): Promise<void> {
  expect(element).toBeInstanceOf(HTMLTextAreaElement);
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await h.settle();
}

describe("SelectField", () => {
  test("shows the current option, lists every option, and hands the chosen option's value to onChange", async () => {
    const h = current = await connected();
    const picked: string[] = [];
    const root = await h.mount(<Chooser picked={picked} />);
    const trigger = root.querySelector<HTMLElement>("[role=combobox]");
    expect(trigger?.textContent).toContain("Studio");
    await act(async () => trigger?.click());
    await h.settle();
    const options = [...document.querySelectorAll<HTMLElement>("[role=option]")];
    expect(options.map((option) => option.textContent)).toEqual(["Studio", "Laptop"]);
    await act(async () => options[1]?.click());
    await h.settle();
    expect(picked).toEqual(["aether:b"]);
    expect(root.querySelector("[role=combobox]")?.textContent).toContain("Laptop");
  });

  test("refuses a value that is not one of its options", async () => {
    const h = current = await connected();
    await expect(h.mount(<SelectField label="Daemon" value="aether:gone" options={DAEMONS} onChange={() => undefined} />)).rejects.toThrow(/holds one of its options/);
  });
});

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
