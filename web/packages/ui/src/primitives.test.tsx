// @vitest-environment happy-dom
import { act, type ReactNode } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { Badge, StatusPill, statusToken, STATUSES } from "./badge.tsx";
import { Button } from "./button.tsx";
import { Dialog, Sheet } from "./dialog.tsx";
import { Field } from "./field.tsx";
import { FOCUSABLE } from "./focus.ts";
import { Menu, nextEnabled, type MenuItem } from "./menu.tsx";
import { byText, click, press, render } from "./render.test-util.tsx";
import { EmptyState, ErrorState, Skeleton } from "./states.tsx";
import { Tabs } from "./tabs.tsx";
import { Toast, ToastRegion } from "./toast.tsx";
import { Tooltip } from "./tooltip.tsx";

afterEach(() => {
  document.body.innerHTML = "";
});

const items = (onSelect = vi.fn()): MenuItem[] => [
  { id: "a", label: "Copy", onSelect },
  { id: "b", label: "Hidden", onSelect, disabled: true },
  { id: "c", label: "Delete", onSelect, tone: "danger" },
];

describe("Button and Field", () => {
  test("a button never submits by default and refuses to be nameless", async () => {
    const { container } = await render(<Button variant="primary">Go</Button>);
    const button = container.querySelector("button");
    expect(button?.type).toBe("button");
    expect(button?.dataset.variant).toBe("primary");
    expect(() => Button({})).toThrow(/visible text or an aria-label/);
  });

  test("a field wires its label, hint and error to the input", async () => {
    const { container } = await render(<Field label="Budget" hint="In USD" error="Enter a number" />);
    const input = container.querySelector("input");
    const label = container.querySelector("label");
    expect(label?.htmlFor).toBe(input?.id);
    expect(input?.getAttribute("aria-invalid")).toBe("true");
    const described = (input?.getAttribute("aria-describedby") ?? "").split(" ").map((id) => document.getElementById(id)?.textContent);
    expect(described).toEqual(["In USD", "Enter a number"]);
  });
});

describe("Menu", () => {
  test("skips disabled items and wraps", () => {
    const list = items();
    expect(nextEnabled(list, 0, 1)).toBe(2);
    expect(nextEnabled(list, 2, 1)).toBe(0);
    expect(nextEnabled(list, 0, -1)).toBe(2);
    expect(nextEnabled([{ id: "x", label: "x", onSelect: () => undefined, disabled: true }], -1, 1)).toBe(-1);
  });

  test("opens on the trigger, moves with arrows, closes with Escape back to the trigger", async () => {
    const { container } = await render(<Menu label="More" items={items()} />);
    const trigger = byText(container, "button", "More");
    await click(trigger);
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(document.activeElement?.textContent).toBe("Copy");
    await press(document.activeElement as Element, "ArrowDown");
    expect(document.activeElement?.textContent).toBe("Delete");
    await press(document.activeElement as Element, "Escape");
    expect(container.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  test("selecting an item runs it once and closes", async () => {
    const onSelect = vi.fn();
    const { container } = await render(<Menu label="More" items={items(onSelect)} />);
    await press(byText(container, "button", "More"), "ArrowUp");
    expect(document.activeElement?.textContent).toBe("Delete");
    await click(document.activeElement as Element);
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(container.querySelector("[role=menu]")).toBeNull();
  });
});

describe("Dialog and Sheet", () => {
  test.each([["dialog", Dialog], ["sheet", Sheet]] as const)("a %s opens modally, is named by its title and closes on Escape", async (kind, Surface) => {
    const onClose = vi.fn();
    const { container, rerender } = await render(<Surface open onClose={onClose} title="Delete run"><p>Gone for good.</p></Surface>);
    const dialog = container.querySelector("dialog");
    expect(dialog?.open).toBe(true);
    expect(dialog?.dataset.kind).toBe(kind);
    expect(document.getElementById(dialog?.getAttribute("aria-labelledby") ?? "")?.textContent).toBe("Delete run");
    await act(async () => dialog?.dispatchEvent(new Event("cancel", { cancelable: true })));
    expect(onClose).toHaveBeenCalledTimes(1);
    await rerender(<Surface open={false} onClose={onClose} title="Delete run" />);
    expect(dialog?.open).toBe(false);
  });
});

describe("Tabs", () => {
  test("arrows select and focus the next tab, and the panel follows", async () => {
    const onSelect = vi.fn();
    const tabs = [{ id: "plan", label: "Plan", panel: "Plan body" }, { id: "limits", label: "Limits", panel: "Limits body" }];
    const { container } = await render(<Tabs label="Run" tabs={tabs} onSelect={onSelect} />);
    const first = byText(container, "[role=tab]", "Plan");
    await press(first, "ArrowRight");
    const second = byText(container, "[role=tab]", "Limits");
    expect(onSelect).toHaveBeenCalledWith("limits");
    expect(document.activeElement).toBe(second);
    expect(second.getAttribute("aria-selected")).toBe("true");
    expect(first.tabIndex).toBe(-1);
    const panel = container.querySelector("[role=tabpanel]");
    expect(panel?.textContent).toBe("Limits body");
    expect(panel?.id).toBe(second.getAttribute("aria-controls"));
    await press(second, "ArrowRight");
    expect(document.activeElement).toBe(first);
  });
});

describe("Toast and Tooltip", () => {
  test("a danger toast is an alert, others are status, and dismiss names the toast", async () => {
    const onDismiss = vi.fn();
    const { container } = await render(<ToastRegion onDismiss={onDismiss} toasts={[{ id: 1, title: "Saved", tone: "success" }, { id: 2, title: "Lost link", tone: "danger" }]} />);
    expect([...container.querySelectorAll(".ui-toast")].map((toast) => toast.getAttribute("role"))).toEqual(["status", "alert"]);
    await click(container.querySelector("[aria-label='Dismiss: Lost link']") as Element);
    expect(onDismiss).toHaveBeenCalledWith(2);
    expect(() => Toast({ toast: { id: 3, title: " " }, onDismiss })).toThrow(/needs a title/);
  });

  test("a tooltip describes its trigger, shows on focus and hides on Escape", async () => {
    const { container } = await render(<Tooltip content="Stops every agent"><Button>Cancel</Button></Tooltip>);
    const trigger = byText(container, "button", "Cancel");
    const tip = container.querySelector("[role=tooltip]") as HTMLElement;
    expect(trigger.getAttribute("aria-describedby")).toBe(tip.id);
    await act(async () => trigger.focus());
    expect(tip.dataset.open).toBe("true");
    await press(trigger, "Escape");
    expect(tip.dataset.open).toBeUndefined();
  });
});

describe("status colours", () => {
  test("every status reads its own §4 token and says its state in words", async () => {
    expect(statusToken("running")).toBe("--status-running");
    expect(statusToken("pr-merged")).toBe("--pr-merged");
    expect(statusToken("mastery-fragile")).toBe("--mastery-fragile");
    expect(new Set(STATUSES.map(statusToken)).size).toBe(STATUSES.length);
    const { container } = await render(<>{STATUSES.map((status) => <StatusPill key={status} status={status} />)}</>);
    const pills = [...container.querySelectorAll<HTMLElement>(".ui-pill")];
    expect(pills.map((pill) => pill.style.getPropertyValue("--pill-color"))).toEqual(STATUSES.map((status) => `var(${statusToken(status)})`));
    expect(pills.every((pill) => (pill.textContent ?? "").trim().length > 0)).toBe(true);
  });
});

// Every primitive with a control: each focusable part takes focus and is covered by the ring rule of base.css.
const INTERACTIVE: ReadonlyArray<readonly [string, ReactNode]> = [
  ["Button", <Button key="b">Go</Button>],
  ["Field", <Field key="f" label="Name" />],
  ["Menu", <Menu key="m" label="More" items={items()} />],
  ["Dialog", <Dialog key="d" open onClose={() => undefined} title="Confirm" footer={<Button>OK</Button>} />],
  ["Sheet", <Sheet key="s" open onClose={() => undefined} title="Details" />],
  ["Tabs", <Tabs key="t" label="Views" tabs={[{ id: "a", label: "A", panel: "a" }, { id: "b", label: "B", panel: "b" }]} />],
  ["Toast", <Toast key="o" toast={{ id: 1, title: "Saved" }} onDismiss={() => undefined} />],
  ["Tooltip", <Tooltip key="p" content="Help"><Button>?</Button></Tooltip>],
  ["EmptyState", <EmptyState key="e" title="Empty" action={<Button>Add</Button>} />],
  ["ErrorState", <ErrorState key="r" message="Failed" onRetry={() => undefined} />],
];

describe("focus is visible on every primitive", () => {
  test.each(INTERACTIVE)("%s", async (_name, node) => {
    const { container } = await render(node);
    const scope = container.querySelector("dialog") ?? container;
    const controls = [...scope.querySelectorAll<HTMLElement>("a[href], button, input, select, textarea, [tabindex], [role=menuitem]")];
    if (container.querySelector("[aria-haspopup=menu]")) {
      await click(container.querySelector("[aria-haspopup=menu]") as Element);
      controls.push(...container.querySelectorAll<HTMLElement>("[role=menuitem]:not(:disabled)"));
    }
    expect(controls.length).toBeGreaterThan(0);
    for (const control of controls) {
      if ((control as HTMLButtonElement).disabled) continue;
      await act(async () => control.focus());
      expect(document.activeElement, control.outerHTML).toBe(control);
      expect(control.matches(`:where(${FOCUSABLE}):focus-visible`), control.outerHTML).toBe(true);
    }
  });

  test("display-only primitives take no focus", async () => {
    const { container } = await render(<><Badge>3</Badge><StatusPill status="queued" /><Skeleton /></>);
    expect(container.querySelectorAll(FOCUSABLE)).toHaveLength(0);
  });
});
