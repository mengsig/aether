// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, test } from "vitest";
import { DraftStore, useDraft } from "./drafts.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const INITIAL = { text: "", count: 0 };

function Form({ store }: { store: DraftStore }) {
  const draft = useDraft("test/form", INITIAL, store);
  return (
    <div>
      <input aria-label="Text" value={draft.value.text} onChange={(event) => draft.update({ text: event.target.value })} />
      <output>{draft.dirty ? "dirty" : "clean"}:{draft.value.count}</output>
      <button onClick={() => draft.update((current) => ({ ...current, count: current.count + 1 }))}>more</button>
      <button onClick={draft.clear}>clear</button>
    </div>
  );
}

async function mount(store: DraftStore) {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => root.render(<Form store={store} />));
  return { container, unmount: async () => { await act(async () => root.unmount()); container.remove(); } };
}

function setValue(input: HTMLInputElement, value: string) {
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("useDraft", () => {
  test("typed text survives an unmount and comes back on the next mount", async () => {
    const store = new DraftStore(null);
    const first = await mount(store);
    const input = first.container.querySelector("input") as HTMLInputElement;
    await act(async () => setValue(input, "hello"));
    expect(first.container.querySelector("output")?.textContent).toBe("dirty:0");
    await first.unmount();
    const second = await mount(store);
    expect((second.container.querySelector("input") as HTMLInputElement).value).toBe("hello");
    await second.unmount();
  });

  test("a functional update sees the current draft, and clear restores the initial values", async () => {
    const store = new DraftStore(null);
    const { container, unmount } = await mount(store);
    const [more, clear] = [...container.querySelectorAll("button")];
    await act(async () => more?.click());
    await act(async () => more?.click());
    expect(container.querySelector("output")?.textContent).toBe("dirty:2");
    await act(async () => clear?.click());
    expect(container.querySelector("output")?.textContent).toBe("clean:0");
    expect(store.keys()).toEqual([]);
    await unmount();
  });

  test("typing back to the initial text forgets the draft", async () => {
    const store = new DraftStore(null);
    const { container, unmount } = await mount(store);
    const input = container.querySelector("input") as HTMLInputElement;
    await act(async () => setValue(input, "a"));
    await act(async () => setValue(input, ""));
    expect(store.keys()).toEqual([]);
    expect(container.querySelector("output")?.textContent).toBe("clean:0");
    await unmount();
  });
});
