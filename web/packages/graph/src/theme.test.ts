// @vitest-environment happy-dom
// A node's status picks its colour token: `mastery-*` from the learner tokens, the rest from `--status-*`.

import { afterEach, describe, expect, test } from "vitest";
import { readTheme } from "./theme.ts";

afterEach(() => { document.body.innerHTML = ""; });

function themedElement(): HTMLElement {
  const element = document.createElement("div");
  element.style.setProperty("--mastery-fragile", "rgb(1, 2, 3)");
  element.style.setProperty("--status-running", "rgb(4, 5, 6)");
  element.style.setProperty("--color-text-muted", "rgb(7, 8, 9)");
  document.body.append(element);
  return element;
}

describe("readTheme status colours", () => {
  test("a mastery status reads its --mastery-* token", () => {
    const theme = readTheme(themedElement());
    expect(theme.status("mastery-fragile")).toBe("rgb(1, 2, 3)");
  });

  test("any other status reads --status-*, and an unknown one falls back to the muted text token", () => {
    const theme = readTheme(themedElement());
    expect(theme.status("running")).toBe("rgb(4, 5, 6)");
    expect(theme.status("mastery-unknown")).toBe("rgb(7, 8, 9)");
    expect(theme.status(undefined)).toBe("rgb(7, 8, 9)");
  });
});
