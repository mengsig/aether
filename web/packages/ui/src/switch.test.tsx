// @vitest-environment happy-dom
import { describe, expect, test } from "vitest";
import { useState } from "react";
import { Segmented, Switch } from "./switch.tsx";
import { click, press, render } from "./render.test-util.tsx";

function SwitchHarness() {
  const [on, setOn] = useState(false);
  return <Switch label="Allow API-billed models" hint="Costs real money" checked={on} onChange={setOn} />;
}

function SegmentedHarness(props: { readonly changed: (id: string) => void }) {
  const [value, setValue] = useState("agent");
  const options = [{ id: "agent", label: "New agent" }, { id: "task", label: "Task" }, { id: "off", label: "Off", disabled: true }];
  return <Segmented label="Work type" value={value} options={options} onChange={(id) => { setValue(id); props.changed(id); }} />;
}

describe("Switch", () => {
  test("is a switch named by its label that toggles on click", async () => {
    const { container, unmount } = await render(<SwitchHarness />);
    const control = container.querySelector("[role=switch]") as HTMLButtonElement;
    expect(control.getAttribute("aria-checked")).toBe("false");
    expect(control.textContent).toContain("Allow API-billed models");
    expect(document.getElementById(control.getAttribute("aria-describedby") ?? "")?.textContent).toBe("Costs real money");
    await click(control);
    expect(control.getAttribute("aria-checked")).toBe("true");
    await unmount();
  });

  test("Space and Enter toggle it once per press, and a disabled switch ignores them", async () => {
    const { container, unmount } = await render(<SwitchHarness />);
    const control = container.querySelector("[role=switch]") as HTMLButtonElement;
    await press(control, " ");
    expect(control.getAttribute("aria-checked")).toBe("true");
    await press(control, "Enter");
    expect(control.getAttribute("aria-checked")).toBe("false");
    await press(control, "Enter", { repeat: true });
    expect(control.getAttribute("aria-checked")).toBe("false");
    await unmount();
    const changes: boolean[] = [];
    const off = await render(<Switch label="Locked" checked={false} disabled onChange={(next) => changes.push(next)} />);
    await press(off.container.querySelector("[role=switch]") as HTMLElement, " ");
    expect(changes).toEqual([]);
    await off.unmount();
  });
});

describe("Segmented", () => {
  test("is a radio group with a roving tabindex; arrows move and choose, skipping disabled segments", async () => {
    const changed: string[] = [];
    const { container, unmount } = await render(<SegmentedHarness changed={(id) => changed.push(id)} />);
    const radios = [...container.querySelectorAll<HTMLButtonElement>("[role=radio]")];
    expect(radios.map((r) => r.tabIndex)).toEqual([0, -1, -1]);
    await press(radios[0] as HTMLElement, "ArrowRight");
    expect(changed).toEqual(["task"]);
    expect(document.activeElement).toBe(radios[1]);
    await press(radios[1] as HTMLElement, "ArrowRight");
    expect(changed).toEqual(["task", "agent"]);
    await click(radios[1] as HTMLElement);
    expect(radios[1]?.getAttribute("aria-checked")).toBe("true");
    await unmount();
  });
});
