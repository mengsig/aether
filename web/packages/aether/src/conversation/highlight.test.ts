// @vitest-environment happy-dom
import { describe, expect, test } from "vitest";
import { paintMatches } from "./highlight.ts";

function list(rows: readonly string[]): HTMLElement {
  const root = document.createElement("div");
  rows.forEach((text, index) => {
    const row = document.createElement("article");
    row.dataset.key = `r${index}`;
    row.textContent = text;
    root.append(row);
  });
  document.body.append(root);
  return root;
}

describe("painting matches", () => {
  test("finds a match after a letter that grows when lowered, at the original offsets", () => {
    const root = list(["İstanbul", "Berlin"]);
    const hit = paintMatches(root, "l", { rowKey: "r0", nth: 0 });
    expect(hit?.textContent).toBe("İstanbul");
    root.remove();
  });

  test("a match inside the grown letter covers that whole letter", () => {
    const root = list(["İ"]);
    expect(() => paintMatches(root, "i", { rowKey: "r0", nth: 0 })).not.toThrow();
    root.remove();
  });
});
