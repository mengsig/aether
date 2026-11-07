// @vitest-environment happy-dom
// Drafts live in memory per agent and target and are saved to sessionStorage, best effort: a
// confirmation clears only the exact text it sent, and a failing store never costs typed text.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, test, vi } from "vitest";
import { invariant } from "../fleet/invariant.ts";
import { draftKey, forgetDrafts, useDraft, type Draft } from "./drafts.ts";

const KEY = draftKey("conn-1", "agt_01JDRAFT0000000000000001", "message");
const mounted: Root[] = [];
afterEach(() => {
  for (const root of mounted.splice(0)) act(() => root.unmount());
  vi.restoreAllMocks();
  sessionStorage.clear();
  forgetDrafts();
});

/** One screen's draft on `key`; `now()` is what it holds after the last render. */
function screen(key = KEY): { readonly now: () => Draft; readonly leave: () => void } {
  let seen: Draft | null = null;
  function Probe() {
    seen = useDraft(key);
    return null;
  }
  const root = createRoot(document.createElement("div"));
  mounted.push(root);
  act(() => root.render(<Probe />));
  return {
    now: () => { invariant(seen !== null, "the probe rendered"); return seen; },
    leave: () => act(() => root.unmount()),
  };
}

const denied = () => { throw new DOMException("denied", "SecurityError"); };

describe("a draft", () => {
  test("outlives its screen, and every screen on its key shows the same text", () => {
    const composer = screen();
    act(() => composer.now().set("half a thought"));
    const other = screen();
    expect(other.now().text).toBe("half a thought");
    composer.leave();
    expect(screen().now().text).toBe("half a thought");
    expect(screen(draftKey("conn-1", "agt_01JDRAFT0000000000000001", "steer")).now().text, "Steer keeps its own").toBe("");
  });

  test("is cleared by a confirmation only while it is still exactly the text sent", () => {
    const composer = screen();
    act(() => composer.now().set("hello"));
    act(() => composer.now().set("hello, and more"));
    act(() => composer.now().delivered("hello"));
    expect(composer.now().text, "text typed since the send is kept").toBe("hello, and more");
    act(() => composer.now().delivered("hello, and more"));
    expect(composer.now().text).toBe("");
    expect(sessionStorage.length, "nothing is left to restore").toBe(0);
  });

  test("is restored from storage after a reload, and a stored value that is not a draft is ignored", () => {
    const before = screen();
    act(() => before.now().set("before the reload"));
    before.leave();
    forgetDrafts();
    expect(screen().now().text).toBe("before the reload");
    const garbled = draftKey("conn-1", "agt_01JDRAFT0000000000000002", "message");
    sessionStorage.setItem(garbled, "{not json");
    expect(screen(garbled).now().text).toBe("");
  });
});

describe("a failing store", () => {
  test.each([{ method: "setItem", store: "full" }, { method: "getItem", store: "unreadable" }] as const)("($store) keeps typed text and says once that reloads lose it", ({ method }) => {
    vi.spyOn(sessionStorage, method).mockImplementation(denied);
    const composer = screen();
    act(() => composer.now().set("still here"));
    expect(composer.now().problem).toBe("Drafts are kept while the app is open, but not across a reload (denied).");
    composer.leave();
    const back = screen();
    expect(back.now().text, "memory keeps the draft across navigation").toBe("still here");
    act(() => back.now().set("still here, longer"));
    act(() => back.now().delivered("still here"));
    expect(back.now().text, "a late confirmation never takes newer text").toBe("still here, longer");
  });
});
