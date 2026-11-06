// @vitest-environment happy-dom
import { describe, expect, test } from "vitest";
import { useState } from "react";
import { Picker } from "./picker.tsx";
import { filterOptions, groupOptions, moveIndex, typeAhead, type PickerOption } from "./picker-model.ts";
import { click, press, render, type } from "./render.test-util.tsx";

const FRUIT: PickerOption[] = [
  { id: "apple", label: "Apple", group: "Pome" },
  { id: "pear", label: "Pear", group: "Pome", description: "Soft when ripe" },
  { id: "plum", label: "Plum", group: "Stone", disabled: true },
  { id: "peach", label: "Peach", group: "Stone", badge: "Default", keywords: "fuzzy" },
];

function Harness(props: { readonly searchable?: boolean; readonly initial?: string | null; readonly onChange?: (id: string) => void }) {
  const [value, setValue] = useState<string | null>(props.initial ?? null);
  return <Picker label="Fruit" value={value} options={FRUIT} searchable={props.searchable} onChange={(id) => { setValue(id); props.onChange?.(id); }} />;
}

const trigger = (root: ParentNode) => root.querySelector<HTMLButtonElement>("[role=combobox]") as HTMLButtonElement;
const options = (root: ParentNode) => [...root.querySelectorAll<HTMLElement>("[role=option]")];
const active = (root: ParentNode) => root.querySelector<HTMLElement>("[role=option][data-active]")?.textContent ?? null;

describe("picker model", () => {
  test("groups keep first-seen order and reject repeated ids", () => {
    expect(groupOptions(FRUIT).map((group) => [group.label, group.options.length])).toEqual([["Pome", 2], ["Stone", 2]]);
    expect(() => groupOptions([...FRUIT, FRUIT[0] as PickerOption])).toThrow(/repeat an id/);
  });
  test("filters by every word across label, description, group and keywords", () => {
    expect(filterOptions(FRUIT, "soft").map((o) => o.id)).toEqual(["pear"]);
    expect(filterOptions(FRUIT, "stone fuzzy").map((o) => o.id)).toEqual(["peach"]);
    expect(filterOptions(FRUIT, "  ").length).toBe(4);
  });
  test("movement skips disabled options and wraps", () => {
    expect(moveIndex(FRUIT, 1, 1)).toBe(3);
    expect(moveIndex(FRUIT, 0, -1)).toBe(3);
    expect(moveIndex([], -1, 1)).toBe(-1);
    expect(moveIndex([{ id: "a", label: "a", disabled: true }], -1, 1)).toBe(-1);
  });
  test("type-ahead matches a prefix and skips disabled options", () => {
    expect(typeAhead(FRUIT, -1, "p")).toBe(1);
    expect(typeAhead(FRUIT, 1, "p")).toBe(3);
    expect(typeAhead(FRUIT, -1, "pl")).toBe(-1);
    expect(typeAhead(FRUIT, -1, "pe")).toBe(1);
    expect(typeAhead(FRUIT, 1, "pe")).toBe(1);
    expect(typeAhead(FRUIT, 1, "pea")).toBe(1);
    expect(typeAhead(FRUIT, 1, "peac")).toBe(3);
    expect(() => typeAhead(FRUIT, 9, "p")).toThrow(/out of range/);
  });
});

describe("Picker", () => {
  test("shows the placeholder, opens on click and lists groups with the chosen option checked", async () => {
    const { container, unmount } = await render(<Harness initial="pear" />);
    expect(trigger(container).textContent).toContain("Pear");
    expect(trigger(container).getAttribute("aria-expanded")).toBe("false");
    await click(trigger(container));
    expect(trigger(container).getAttribute("aria-expanded")).toBe("true");
    expect([...container.querySelectorAll(".ui-picker-group-label")].map((n) => n.textContent)).toEqual(["Pome", "Stone"]);
    expect(options(container).find((o) => o.getAttribute("aria-selected") === "true")?.textContent).toContain("Pear");
    expect(active(container)).toContain("Pear");
    await unmount();
  });

  test("arrows move past disabled options, Enter chooses, Escape closes and refocuses the trigger", async () => {
    const chosen: string[] = [];
    const { container, unmount } = await render(<Harness onChange={(id) => chosen.push(id)} />);
    await press(trigger(container), "ArrowDown");
    const list = container.querySelector("[role=listbox]") as HTMLElement;
    expect(document.activeElement).toBe(list);
    expect(active(container)).toContain("Apple");
    await press(list, "ArrowDown");
    await press(list, "ArrowDown");
    expect(active(container)).toContain("Peach");
    await press(list, "Enter");
    expect(chosen).toEqual(["peach"]);
    expect(container.querySelector("[role=listbox]")).toBeNull();
    expect(document.activeElement).toBe(trigger(container));
    await press(trigger(container), " ");
    await press(container.querySelector("[role=listbox]") as HTMLElement, "Escape");
    expect(container.querySelector("[role=listbox]")).toBeNull();
    expect(document.activeElement).toBe(trigger(container));
    await unmount();
  });

  test("type-ahead on the open list jumps to the first match", async () => {
    const { container, unmount } = await render(<Harness />);
    await click(trigger(container));
    const list = container.querySelector("[role=listbox]") as HTMLElement;
    await press(list, "p");
    expect(active(container)).toContain("Pear");
    await press(list, "e");
    expect(active(container)).toContain("Pear");
    await unmount();
  });

  test("a searchable picker focuses its search box and filters as you type", async () => {
    const { container, unmount } = await render(<Harness searchable />);
    await click(trigger(container));
    const search = container.querySelector<HTMLInputElement>("[role=searchbox]") as HTMLInputElement;
    expect(document.activeElement).toBe(search);
    await type(search, "soft");
    expect(options(container).map((o) => o.textContent)).toEqual([expect.stringContaining("Pear")]);
    await type(search, "zzz");
    expect(container.textContent).toContain("Nothing matches");
    await unmount();
  });

  test("Home and End jump to the first and last enabled options", async () => {
    const { container, unmount } = await render(<Harness initial="pear" />);
    await press(trigger(container), "Enter");
    const list = container.querySelector("[role=listbox]") as HTMLElement;
    expect(active(container)).toContain("Pear");
    await press(list, "End");
    expect(active(container)).toContain("Peach");
    await press(list, "Home");
    expect(active(container)).toContain("Apple");
    expect(list.getAttribute("aria-activedescendant")).toBe(options(container)[0]?.id);
    await unmount();
  });

  test("type-ahead on the closed trigger chooses the next match without opening", async () => {
    const chosen: string[] = [];
    const { container, unmount } = await render(<Harness initial="apple" onChange={(id) => chosen.push(id)} />);
    await press(trigger(container), "p");
    expect(chosen).toEqual(["pear"]);
    expect(container.querySelector("[role=listbox]")).toBeNull();
    await press(trigger(container), "p");
    expect(chosen).toEqual(["pear", "peach"]);
    await unmount();
  });

  test("search narrows the list; arrows and Enter choose among the matches; Escape closes", async () => {
    const chosen: string[] = [];
    const { container, unmount } = await render(<Harness searchable onChange={(id) => chosen.push(id)} />);
    await click(trigger(container));
    const search = container.querySelector<HTMLInputElement>("[role=searchbox]") as HTMLInputElement;
    await type(search, "pe");
    expect(options(container).map((o) => o.textContent)).toEqual([expect.stringContaining("Pear"), expect.stringContaining("Peach")]);
    expect(active(container)).toContain("Pear");
    await press(search, "ArrowDown");
    expect(search.getAttribute("aria-activedescendant")).toBe(options(container)[1]?.id);
    await press(search, "Enter");
    expect(chosen).toEqual(["peach"]);
    await click(trigger(container));
    await press(container.querySelector("[role=searchbox]") as HTMLElement, "Escape");
    expect(container.querySelector("[role=listbox]")).toBeNull();
    expect(document.activeElement).toBe(trigger(container));
    await unmount();
  });

  test("keyboard order follows the drawn groups even when the options interleave them", async () => {
    const mixed: PickerOption[] = [{ id: "a", label: "Ant", group: "Small" }, { id: "b", label: "Bear", group: "Large" }, { id: "c", label: "Cat", group: "Small" }];
    const { container, unmount } = await render(<Picker label="Animal" value={null} options={mixed} onChange={() => undefined} />);
    await press(trigger(container), "ArrowDown");
    const list = container.querySelector("[role=listbox]") as HTMLElement;
    const drawn = options(container).map((o) => o.textContent);
    expect(drawn).toEqual([expect.stringContaining("Ant"), expect.stringContaining("Cat"), expect.stringContaining("Bear")]);
    await press(list, "ArrowDown");
    expect(active(container)).toContain("Cat");
    expect(list.getAttribute("aria-activedescendant")).toBe(options(container)[1]?.id);
    await unmount();
  });

  test("a disabled option is not chosen on click; an enabled one is", async () => {
    const chosen: string[] = [];
    const { container, unmount } = await render(<Harness onChange={(id) => chosen.push(id)} />);
    await click(trigger(container));
    await click(options(container)[2] as HTMLElement);
    expect(chosen).toEqual([]);
    await click(options(container)[0] as HTMLElement);
    expect(chosen).toEqual(["apple"]);
    expect(trigger(container).textContent).toContain("Apple");
    await unmount();
  });

  test("hint and error are wired through aria-describedby", async () => {
    const { container, unmount } = await render(<Picker label="Fruit" value={null} options={FRUIT} onChange={() => undefined} hint="Pick one" error="Required" />);
    const described = trigger(container).getAttribute("aria-describedby") ?? "";
    expect(described.split(" ").length).toBe(2);
    expect(trigger(container).getAttribute("aria-invalid")).toBe("true");
    await unmount();
  });
});
