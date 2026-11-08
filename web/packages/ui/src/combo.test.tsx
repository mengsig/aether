// @vitest-environment happy-dom
import { describe, expect, test } from "vitest";
import { useState } from "react";
import { Combo } from "./combo.tsx";
import { click, press, render, type } from "./render.test-util.tsx";

const ZONES = [{ id: "Europe/Copenhagen", label: "Europe/Copenhagen" }, { id: "Europe/Berlin", label: "Europe/Berlin" }];

function Harness(props: { readonly picked?: (id: string) => void }) {
  const [text, setText] = useState("");
  const shown = ZONES.filter((zone) => zone.id.toLowerCase().includes(text.toLowerCase()));
  return <Combo label="Timezone" value={text} onChange={setText} suggestions={shown} onPick={(option) => props.picked?.(option.id)} emptyText="No such zone" />;
}

describe("Combo", () => {
  test("the text is the value; suggestions appear on focus and a click fills the text in", async () => {
    const picked: string[] = [];
    const { container, unmount } = await render(<Harness picked={(id) => picked.push(id)} />);
    const input = container.querySelector("input") as HTMLInputElement;
    expect(input.getAttribute("role")).toBe("combobox");
    await type(input, "ber");
    expect(input.value).toBe("ber");
    const options = [...container.querySelectorAll("[role=option]")];
    expect(options.map((o) => o.textContent)).toEqual(["Europe/Berlin"]);
    await click(options[0] as HTMLElement);
    expect(input.value).toBe("Europe/Berlin");
    expect(picked).toEqual(["Europe/Berlin"]);
    expect(container.querySelector("[role=listbox]")).toBeNull();
    await unmount();
  });

  test("arrows pick a suggestion with Enter; Escape closes and keeps the typed text", async () => {
    const { container, unmount } = await render(<Harness />);
    const input = container.querySelector("input") as HTMLInputElement;
    await type(input, "Europe");
    await press(input, "ArrowDown");
    await press(input, "ArrowDown");
    expect(container.querySelector("[role=option][data-active]")?.textContent).toBe("Europe/Berlin");
    await press(input, "Enter");
    expect(input.value).toBe("Europe/Berlin");
    await type(input, "Eur");
    await press(input, "Escape");
    expect(input.value).toBe("Eur");
    expect(container.querySelector("[role=listbox]")).toBeNull();
    await unmount();
  });

  test("no suggestion shows the empty text instead of a silent nothing", async () => {
    const { container, unmount } = await render(<Harness />);
    await type(container.querySelector("input") as HTMLInputElement, "mars");
    expect(container.textContent).toContain("No such zone");
    await unmount();
  });
});
