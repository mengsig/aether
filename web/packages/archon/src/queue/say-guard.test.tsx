// @vitest-environment happy-dom
import { act, useState } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SayDialog } from "./NewItemDialog.tsx";
import { archonHarness, byText, click, CONN, type, type ArchonHarness } from "./test/harness.tsx";

const change = vi.hoisted(() => vi.fn());
vi.mock("./archon.ts", async (importOriginal) => ({ ...await importOriginal<typeof import("./archon.ts")>(), useChange: () => change }));

const access = { conn: CONN, ready: true, live: true, has: () => true, refusal: null, speakRefusal: null };
let h: ArchonHarness | undefined;
beforeEach(() => { vi.useFakeTimers(); change.mockReset(); });
afterEach(() => { h?.stop(); h = undefined; document.body.innerHTML = ""; vi.useRealTimers(); });

function Screen() {
  const [open, setOpen] = useState(true);
  return <><button onClick={() => setOpen(true)}>Open</button><SayDialog access={access} open={open} onClose={() => setOpen(false)} /></>;
}

function deferred() {
  let resolve!: (value: { ok: true; reply: Record<string, unknown> } | { ok: false; error: string }) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<{ ok: true; reply: Record<string, unknown> } | { ok: false; error: string }>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function setup() {
  h = await archonHarness({ protocol: 24 });
  const root = await h.mount(<Screen />);
  await type(h, document.querySelector("dialog textarea") as Element, "First message");
  return { h, root };
}

function submitTwice() {
  const form = document.querySelector("dialog form");
  if (!form) throw new Error("message form is missing");
  form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
}

it("guards rapid submissions and close/reopen while preserving edits on success", async () => {
  const pending = deferred();
  change.mockReturnValueOnce(pending.promise).mockResolvedValueOnce({ ok: true, reply: {} });
  const { h, root } = await setup();
  await act(async () => submitTwice());
  expect(change).toHaveBeenCalledTimes(1);
  expect(change).toHaveBeenCalledWith({ cmd: "say", text: "First message" });
  expect(byText<HTMLButtonElement>(document, "dialog button", "Sending…").disabled).toBe(true);
  await click(h, byText(document, "dialog button", "Close"));
  await click(h, byText(root, "button", "Open"));
  await act(async () => submitTwice());
  expect(change).toHaveBeenCalledTimes(1);
  await type(h, document.querySelector("dialog textarea") as Element, "Edited while sending");
  await act(async () => pending.resolve({ ok: true, reply: {} }));
  expect(document.querySelector<HTMLTextAreaElement>("dialog textarea")?.value).toBe("Edited while sending");
  expect(document.querySelector("dialog [role=status]")?.textContent).toBe("Sent to the orchestrator.");
  await click(h, byText(document, "dialog button", "Send"));
  expect(change).toHaveBeenCalledTimes(2);
  expect(document.querySelector<HTMLTextAreaElement>("dialog textarea")?.value).toBe("");
});

it.each(["result", "throw"])("shows a %s failure, retains the draft, and allows retry", async (failure) => {
  const pending = deferred();
  change.mockReturnValueOnce(pending.promise).mockResolvedValueOnce({ ok: true, reply: {} });
  const { h } = await setup();
  await act(async () => submitTwice());
  await act(async () => {
    if (failure === "throw") pending.reject(new Error("Send failed"));
    else pending.resolve({ ok: false, error: "Send failed" });
  });
  expect(change).toHaveBeenCalledTimes(1);
  expect(document.querySelector("dialog [role=alert]")?.textContent).toBe("Send failed");
  expect(document.querySelector<HTMLTextAreaElement>("dialog textarea")?.value).toBe("First message");
  await click(h, byText(document, "dialog button", "Send"));
  expect(change).toHaveBeenCalledTimes(2);
  expect(document.querySelector<HTMLTextAreaElement>("dialog textarea")?.value).toBe("");
  expect(document.querySelector("dialog [role=alert]")).toBeNull();
});
