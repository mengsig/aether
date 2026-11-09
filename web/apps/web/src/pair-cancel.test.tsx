// @vitest-environment happy-dom
import { AppProvider, createApp } from "@aether-ui/core";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import PairScreen from "./pair-screen.tsx";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

it("cancels a claim and keeps the draft", async () => {
  const fetcher = vi.fn(() => new Promise<Response>(() => {}));
  const storage = { get: async () => undefined, set: async () => {} };
  const app = createApp({ kind: "browser", storage }, { endpoints: [], fetch: fetcher });
  const container = document.body.appendChild(document.createElement("main"));
  const root = createRoot(container);
  await act(async () => root.render(<AppProvider app={app}><PairScreen params={{ daemon: "archon" }} /></AppProvider>));
  const input = container.querySelector("input");
  if (!input) throw new Error("Missing pairing code input");
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setter) throw new Error("Missing input value setter");
    setter.call(input, "ABCD");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const button = (name: string) => {
    const found = [...container.querySelectorAll("button")].find((item) => item.textContent === name);
    if (!found) throw new Error(`Missing button ${name}`);
    return found;
  };
  await act(async () => button("Pair").click());
  expect(button("Cancel")).toBeTruthy();
  await act(async () => button("Cancel").click());
  expect(button("Pair")).toBeTruthy();
  expect(input.value).toBe("ABCD");
  expect((fetcher.mock.calls[0] as unknown as [string, RequestInit])[1].signal?.aborted).toBe(true);
  await act(async () => root.unmount());
  container.remove();
});
