// Renders into a DOM container under act(), for the kit's DOM tests (happy-dom).

import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export interface Rendered {
  readonly container: HTMLElement;
  rerender(node: ReactNode): Promise<void>;
  unmount(): Promise<void>;
}

export async function render(node: ReactNode): Promise<Rendered> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => root.render(node));
  return {
    container,
    rerender: async (next) => act(async () => root.render(next)),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

export async function press(target: Element, key: string, init: KeyboardEventInit = {}): Promise<void> {
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));
  });
}

export async function click(target: Element): Promise<void> {
  await act(async () => (target as HTMLElement).click());
}

export async function type(input: HTMLInputElement, value: string): Promise<void> {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    setter?.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

export function byText(container: ParentNode, selector: string, text: string): HTMLElement {
  const found = [...container.querySelectorAll<HTMLElement>(selector)].find((element) => element.textContent?.includes(text));
  if (!found) throw new Error(`no ${selector} with text ${text}`);
  return found;
}
