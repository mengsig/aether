// @vitest-environment happy-dom
// Drafts shared by screens on one key (the composer and Steer): a late confirmation clears only the
// text it sent, never another screen's newer draft, and an unreadable store is always said.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, test, vi } from "vitest";
import { invariant } from "../fleet/invariant.ts";
import { draftKey, readDraft, useDraft, type Draft } from "./drafts.ts";

const KEY = draftKey("conn-1", "agt_01JDRAFT0000000000000001", "message");
const mounted: Root[] = [];
afterEach(() => {
  for (const root of mounted.splice(0)) act(() => root.unmount());
  vi.restoreAllMocks();
  sessionStorage.clear();
});

/** One screen's draft on KEY; `now()` is what it holds after the last render. */
function screen(): { readonly now: () => Draft } {
  let seen: Draft | null = null;
  function Probe() {
    seen = useDraft(KEY);
    return null;
  }
  const root = createRoot(document.createElement("div"));
  mounted.push(root);
  act(() => root.render(<Probe />));
  return { now: () => { invariant(seen !== null, "the probe rendered"); return seen; } };
}

const denied = () => { throw new DOMException("denied", "SecurityError"); };

describe("a draft shared by two screens", () => {
  test("a confirmation after a failed write leaves another screen's newer draft", () => {
    const composer = screen();
    const write = vi.spyOn(sessionStorage, "setItem").mockImplementationOnce(denied);
    act(() => composer.now().set("hello there"));
    expect(composer.now().problem, "the failed write is said").toContain("This draft is not kept if you leave");
    write.mockRestore();
    const steer = screen();
    act(() => steer.now().set("hi"));
    act(() => composer.now().delivered("hello there"));
    expect(readDraft(KEY), "Steer's newer draft is kept").toBe("hi");
  });

  test("a confirmation after a failed write clears this screen's own stale copy", () => {
    const composer = screen();
    act(() => composer.now().set("hel"));
    vi.spyOn(sessionStorage, "setItem").mockImplementationOnce(denied);
    act(() => composer.now().set("hello"));
    expect(readDraft(KEY), "the store still holds the older text").toBe("hel");
    act(() => composer.now().delivered("hello"));
    expect(composer.now().text).toBe("");
    expect(readDraft(KEY), "the stale copy is not offered again").toBe("");
  });

  test("a store that cannot be read is said after any write, failed or not", () => {
    vi.spyOn(sessionStorage, "getItem").mockImplementation(denied);
    const composer = screen();
    expect(composer.now().problem).toBe("This draft is not kept if you leave (the store cannot be read)");
    const write = vi.spyOn(sessionStorage, "setItem").mockImplementationOnce(denied);
    act(() => composer.now().set("a"));
    expect(composer.now().problem).toBe("This draft is not kept if you leave (denied)");
    write.mockRestore();
    act(() => composer.now().set("ab"));
    expect(composer.now().problem, "reading still fails, so the draft is still not kept").toBe("This draft is not kept if you leave (the store cannot be read)");
  });
});
