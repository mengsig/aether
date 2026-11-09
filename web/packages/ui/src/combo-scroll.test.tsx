// @vitest-environment happy-dom
import { act, useState } from "react";
import { describe, expect, test, vi } from "vitest";
import { Combo } from "./combo.tsx";
import type { PickerOption } from "./picker-model.ts";
import { press, render } from "./render.test-util.tsx";

const OPTIONS: PickerOption[] = Array.from({ length: 40 }, (_, index) => ({
  id: `option-${index}`, label: `Option ${index}`, disabled: index === 1 || index === 39,
}));

function Harness({ suggestions = OPTIONS }: { readonly suggestions?: readonly PickerOption[] }) {
  const [value, setValue] = useState("");
  return <Combo label="Choices" value={value} onChange={setValue} suggestions={suggestions} emptyText="No choices" />;
}

describe("Combo scrolling", () => {
  test("keyboard navigation scrolls the active option, skips disabled options, wraps and picks", async () => {
    const calls: { node: HTMLElement; owner: Element | null; options: ScrollIntoViewOptions | boolean | undefined }[] = [];
    const scroll = vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(function (this: HTMLElement, options) {
      calls.push({ node: this, owner: this.closest(".ui-combo"), options });
    });
    const { container, unmount } = await render(<><Harness /><Combo label="Other" value="" onChange={() => {}} suggestions={OPTIONS} /></>);
    const inputs = container.querySelectorAll<HTMLInputElement>("input");
    const input = inputs[0] as HTMLInputElement;
    await act(async () => input.focus());
    expect(calls).toHaveLength(0);
    await press(input, "ArrowDown");
    expect(calls.at(-1)?.node.textContent).toBe("Option 0");
    await press(input, "ArrowDown");
    expect(calls.at(-1)?.node.textContent).toBe("Option 2");
    for (let step = 0; step < 30; step += 1) await press(input, "ArrowDown");
    expect(calls.at(-1)?.node.textContent).toBe("Option 32");
    await press(input, "ArrowUp");
    expect(calls.at(-1)?.node.textContent).toBe("Option 31");
    await press(input, "Escape");
    await press(input, "ArrowDown");
    expect(calls.at(-1)?.node.textContent).toBe("Option 32");
    for (let step = 0; step < 7; step += 1) await press(input, "ArrowDown");
    expect(calls.at(-1)?.node.textContent).toBe("Option 0");
    await press(input, "ArrowUp");
    expect(calls.at(-1)?.node.textContent).toBe("Option 38");
    expect(calls.every(({ owner, options }) => owner?.contains(input) && JSON.stringify(options) === JSON.stringify({ block: "nearest" }))).toBe(true);
    expect(input.getAttribute("aria-activedescendant")).toBe(calls.at(-1)?.node.id);
    expect(document.activeElement).toBe(input);
    await press(input, "Enter");
    expect(input.value).toBe("option-38");
    expect(input.getAttribute("aria-expanded")).toBe("false");
    await unmount();
    scroll.mockRestore();
  });

  test("shrinking suggestions and lists with no enabled option do not scroll a missing item", async () => {
    const scroll = vi.spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(() => {});
    const { container, rerender, unmount } = await render(<Harness />);
    const input = container.querySelector("input") as HTMLInputElement;
    await press(input, "ArrowUp");
    expect(scroll).toHaveBeenCalledTimes(1);
    await rerender(<Harness suggestions={[]} />);
    expect(input.hasAttribute("aria-activedescendant")).toBe(false);
    await press(input, "ArrowDown");
    await rerender(<Harness suggestions={[{ id: "disabled", label: "Disabled", disabled: true }]} />);
    await press(input, "ArrowDown");
    expect(scroll).toHaveBeenCalledTimes(1);
    await unmount();
    scroll.mockRestore();
  });
});
