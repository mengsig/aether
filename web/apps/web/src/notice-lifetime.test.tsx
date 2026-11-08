// @vitest-environment happy-dom
import type { ToastMessage } from "@aether-ui/ui";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { useNoticeLifetime } from "./notice-lifetime.ts";
import { NOTICE_MS } from "./notices.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const toast = (id: number): ToastMessage => ({ id, title: `failure ${id}`, tone: "danger" });
function Lifetimes({ notices, dismiss }: { notices: readonly ToastMessage[]; dismiss: (id: ToastMessage["id"]) => void }) {
  useNoticeLifetime(notices, dismiss);
  return null;
}

let root: Root;
const dismiss = vi.fn();
beforeEach(() => { vi.useFakeTimers(); dismiss.mockClear(); root = createRoot(document.body.appendChild(document.createElement("div"))); });
afterEach(async () => { await act(async () => root.unmount()); vi.useRealTimers(); });

const show = async (notices: readonly ToastMessage[]) => act(async () => root.render(<Lifetimes notices={notices} dismiss={dismiss} />));
const pass = async (ms: number) => act(async () => { vi.advanceTimersByTime(ms); });

test("a toast leaves NOTICE_MS after it appeared, however many others arrive meanwhile", async () => {
  await show([toast(1)]);
  await pass(NOTICE_MS - 2_000);
  await show([toast(1), toast(2)]);
  await pass(1_000);
  await show([toast(1), toast(2), toast(3)]);
  await pass(1_000);
  expect(dismiss.mock.calls, "the first toast leaves at its own NOTICE_MS").toEqual([[1]]);
  await pass(NOTICE_MS - 2_000);
  expect(dismiss.mock.calls, "the second toast lived its full NOTICE_MS").toEqual([[1], [2]]);
  await pass(1_000);
  expect(dismiss.mock.calls).toEqual([[1], [2], [3]]);
});

test("a toast that was dismissed or folded away before its time is not dismissed again", async () => {
  await show([toast(1), toast(2)]);
  await show([toast(2)]);
  await pass(NOTICE_MS);
  expect(dismiss.mock.calls).toEqual([[2]]);
  await show([]);
  await pass(NOTICE_MS * 2);
  expect(dismiss).toHaveBeenCalledTimes(1);
});
