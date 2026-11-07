// @vitest-environment happy-dom
// The app window's native side: outside links go to main with visible feedback (by mouse or
// keyboard), a route main sends navigates, and an alert's click lands on its question.

import { DEFAULT_ALERT_SETTINGS, DesktopAlerts } from "@aether-ui/core";
import { afterEach, describe, expect, test, vi } from "vitest";
import { focusQuestion, followDeepLinks, interceptOutsideLinks, openOutside, outsideHref } from "./desktop.ts";

const BASE = "app://aether/index.html";

function anchor(href: string, text = "Open"): HTMLAnchorElement {
  const link = document.body.appendChild(document.createElement("a"));
  link.href = href;
  link.target = "_blank";
  link.append(document.createElement("span"));
  link.firstElementChild?.append(text);
  return link;
}

afterEach(() => {
  document.body.replaceChildren();
  location.hash = "";
});

describe("outsideHref", () => {
  test("outside links resolve, from the anchor or anything inside it", () => {
    expect(outsideHref(anchor("https://github.com/o/r/pull/1").firstElementChild, BASE)).toBe("https://github.com/o/r/pull/1");
    expect(outsideHref(anchor("http://127.0.0.1:4387/session/x"), BASE)).toBe("http://127.0.0.1:4387/session/x");
  });

  test("in-app links and no link at all stay with the app", () => {
    expect(outsideHref(anchor("#/inbox"), BASE)).toBeNull();
    expect(outsideHref(anchor("app://aether/other.html"), BASE)).toBeNull();
    expect(outsideHref(document.body, BASE)).toBeNull();
    expect(outsideHref(null, BASE)).toBeNull();
  });

  test("a download the app made itself stays with the app; another origin's blob does not", () => {
    const download = anchor("blob:app://aether/5f0c6a1e-0d3c-4b1a-9a57-4f1e3c2b7d10");
    download.download = "memory.json";
    expect(outsideHref(download, BASE)).toBeNull();
    expect(outsideHref(anchor("blob:https://evil.example/5f0c"), BASE)).toBe("blob:https://evil.example/5f0c");
  });
});

describe("openOutside", () => {
  test("success and failure both say what happened; a cancel says nothing", async () => {
    const say = vi.fn();
    const openExternal = vi.fn(async () => {});
    await openOutside({ openExternal }, "https://github.com/o/r", say);
    await openOutside({ openExternal }, "http://127.0.0.1:4387/r", say);
    openExternal.mockRejectedValueOnce(new Error("The app opens https: links only, not file:"));
    await openOutside({ openExternal }, "file:///etc/passwd", say);
    openExternal.mockRejectedValueOnce(new Error("The report at http://127.0.0.1:4387 did not load; is its server still running?"));
    await openOutside({ openExternal }, "http://127.0.0.1:4387/r", say);
    openExternal.mockRejectedValueOnce(new Error("Cancelled"));
    await openOutside({ openExternal }, "https://docs.example/x", say);
    expect(say.mock.calls).toEqual([
      ["Opened github.com in your browser", "success"],
      ["The report opened in its own window", "success"],
      ["Could not open file:///etc/passwd: The app opens https: links only, not file:", "danger"],
      ["Could not open the report: The report at http://127.0.0.1:4387 did not load; is its server still running?", "danger"],
    ]);
  });
});

describe("interceptOutsideLinks", () => {
  test("a click, a middle click and Enter on an outside link go to main, not a new window", () => {
    const open = vi.fn();
    const stop = interceptOutsideLinks(document, open);
    const link = anchor("https://github.com/o/r");
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    link.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    link.dispatchEvent(new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }));
    link.focus();
    link.click();
    expect(open.mock.calls).toEqual([["https://github.com/o/r"], ["https://github.com/o/r"], ["https://github.com/o/r"]]);
    stop();
    link.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(open).toHaveBeenCalledTimes(3);
  });

  test("in-app links, right clicks and handled clicks are left alone", () => {
    const open = vi.fn();
    const stop = interceptOutsideLinks(document, open);
    const leftAlone: boolean[] = [];
    const record = (event: Event) => { leftAlone.push(!event.defaultPrevented); event.preventDefault(); };
    document.addEventListener("click", record);
    anchor("#/inbox").dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(leftAlone).toEqual([true]);
    document.removeEventListener("click", record);
    anchor("https://github.com/o/r").dispatchEvent(new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 2 }));
    const handled = anchor("https://github.com/o/r");
    handled.addEventListener("click", (event) => event.preventDefault());
    handled.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(open).not.toHaveBeenCalled();
    stop();
  });
});

describe("deep links and alert clicks", () => {
  function question(anchorValue: string): HTMLTextAreaElement {
    const item = document.body.appendChild(document.createElement("li"));
    item.dataset.question = anchorValue;
    item.scrollIntoView = vi.fn();
    return item.appendChild(document.createElement("textarea"));
  }

  test("a route from main navigates the app", () => {
    let deliver: (route: string) => void = () => {};
    const alerts = new DesktopAlerts({ desktop: { notify: vi.fn() }, settings: async () => DEFAULT_ALERT_SETTINGS, isFocused: () => false });
    followDeepLinks({ onDeepLink: (callback) => { deliver = callback; return () => {}; } }, alerts, document);
    deliver("/archon/queue");
    expect(location.hash).toBe("#/archon/queue");
    expect(() => deliver("fleet")).toThrow(/app routes/);
  });

  test("a click on a question alert opens the inbox on that alert's own question", async () => {
    let deliver: (route: string) => void = () => {};
    const notify = vi.fn<(note: { title: string; body: string; route?: string }) => Promise<void>>(async () => {});
    const alerts = new DesktopAlerts({ desktop: { notify }, settings: async () => DEFAULT_ALERT_SETTINGS, isFocused: () => false });
    followDeepLinks({ onDeepLink: (callback) => { deliver = callback; return () => {}; } }, alerts, document);
    const asked = (conn: string) => ({ key: `q:${conn}/a1:1`, kind: "question" as const, title: "A asks", body: "Which?", anchor: `${conn}/a1:1` });
    expect(await alerts.deliver([asked("c1"), asked("c2")])).toBe(2);
    const first = question("c1/a1:1");
    const second = question("c2/a1:1");
    const [toFirst, toSecond] = notify.mock.calls.map(([note]) => note.route ?? "");
    deliver(toSecond ?? "");
    expect(location.hash).toBe("#/inbox");
    expect(document.activeElement, "the later alert's question, on its own daemon").toBe(second);
    deliver(toFirst ?? "");
    expect(document.activeElement, "the earlier alert still finds its own question").toBe(first);
  });

  test("focusQuestion waits for the entry to render, then puts the cursor in its answer", () => {
    const frames: Array<() => void> = [];
    focusQuestion(document, "a1:2", (step) => frames.push(step));
    expect(frames).toHaveLength(1);
    question("a1:1");
    const answer = question("a1:2");
    frames.shift()?.();
    expect(document.activeElement).toBe(answer);
    expect(answer.parentElement?.scrollIntoView).toHaveBeenCalledWith({ block: "center" });
  });

  test("focusQuestion stops once its tries run out", () => {
    const frames: Array<() => void> = [];
    focusQuestion(document, "gone:1", (step) => frames.push(step), 2);
    while (frames.length > 0) frames.shift()?.();
    expect(document.activeElement).toBe(document.body);
    expect(() => focusQuestion(document, "", () => {})).toThrow();
  });
});
