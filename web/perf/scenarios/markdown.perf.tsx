import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterAll, expect, test } from "vitest";
import { Markdown } from "../../packages/ui/src/markdown/markdown.tsx";
import { Results, timed } from "../measure.tsx";

const results = new Results("markdown");
afterAll(() => results.write());

test("renders a dense 200 KB message", async () => {
  const dense = "Some **bold** text with `code`, a [link](https://example.com/x) and https://auto.example/y.\nsecond line\n\n- item *one*\n- item two\n\n> quote\n\n";
  const text = dense.repeat(Math.ceil(200_000 / dense.length));
  let root: Root | undefined;
  let container = document.createElement("div");
  const elapsed = await timed(async () => {
    root = createRoot(container);
    await act(async () => root?.render(<Markdown text={text} />));
  }, async () => {
    if (root) await act(async () => root?.unmount());
    document.body.innerHTML = "";
    container = document.createElement("div");
    document.body.append(container);
  });
  expect(container.querySelectorAll("a").length).toBeGreaterThan(1000);
  if (root) await act(async () => root?.unmount());
  results.time("markdown.dense.ms", elapsed);
});
