// @vitest-environment happy-dom
// The session composer keeps what is typed per session, and a send that finishes late clears only
// the text it sent: whatever was typed meanwhile, here or after coming back, stays.

import { drafts } from "@aether-ui/core";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, expect, test } from "vitest";
import { Composer } from "./Composer.tsx";
import { draftKey } from "./model.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SESSION = "nses_composer";
const KEY = draftKey(SESSION, "reply");
const roots: Root[] = [];
afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  drafts.clear(KEY);
});

function deferredSend() {
  let finish: (failure: string | null) => void = () => { throw new Error("nothing sent yet"); };
  const sent: string[] = [];
  const onSend = (text: string) => new Promise<string | null>((resolve) => { sent.push(text); finish = resolve; });
  return { onSend, sent, finish: (failure: string | null) => finish(failure) };
}

async function mount(onSend: (text: string) => Promise<string | null>): Promise<HTMLTextAreaElement> {
  const container = document.body.appendChild(document.createElement("div"));
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(<Composer sessionId={SESSION} waiting={false} problem={false} disabled={false} onSend={onSend} />));
  const area = container.querySelector("textarea");
  if (!(area instanceof HTMLTextAreaElement)) throw new Error("the composer renders its textarea");
  return area;
}

async function type(area: HTMLTextAreaElement, value: string): Promise<void> {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(area, value);
    area.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

const send = (area: HTMLTextAreaElement) => act(async () => { area.form?.requestSubmit(); });

test("a send that finishes after the person came back and typed again keeps the newer text", async () => {
  const first = deferredSend();
  const area = await mount(first.onSend);
  await type(area, "reply A");
  await send(area);
  expect(first.sent).toEqual(["reply A"]);
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  const again = await mount(first.onSend);
  expect(again.value).toBe("reply A");
  await type(again, "reply B");
  await act(async () => { first.finish(null); });
  expect(again.value).toBe("reply B");
  expect(drafts.get(KEY)).toEqual({ text: "reply B" });
});

test("a send that finishes with the text unchanged clears it, and a failed send keeps it", async () => {
  const outcome = deferredSend();
  const area = await mount(outcome.onSend);
  await type(area, "reply A");
  await send(area);
  await act(async () => { outcome.finish("the teacher is away"); });
  expect(area.value).toBe("reply A");
  await send(area);
  await act(async () => { outcome.finish(null); });
  expect(area.value).toBe("");
  expect(drafts.get(KEY)).toBeUndefined();
});
