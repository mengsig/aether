// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import { CodeBlock } from "./code.tsx";
import { Markdown } from "./markdown.tsx";
import { renderMath } from "./math-render.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

test("renders 200 mixed lesson messages (over 200 KB) including math within 5 seconds", async () => {
  const lesson = "# Conservation\n\n**Energy** and *mass* satisfy $E=mc^2$. Read [the notes](https://example.test).\n\n"
    + "- First principle\n- Second principle\n\n| Quantity | Unit |\n| --- | --- |\n| Energy | J |\n\n"
    + "```ts\nconst mass = 2; // sample\nreturn mass;\n```\n\n$$\\frac{a}{b}$$\n\n"
    + "A lesson connects a worked example to an explanation and a short practice exercise. ".repeat(10);
  expect(lesson.length * 200).toBeGreaterThan(200_000);
  renderMath("x^2", false);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    const start = performance.now();
    await act(async () => { root.render(<>{Array.from({ length: 200 }, (_, i) => <Markdown key={i} text={lesson} />)}</>); });
    await act(async () => { await import("./math-render.ts"); });
    const elapsed = performance.now() - start;
    expect(container.querySelectorAll(".md")).toHaveLength(200);
    expect(container.querySelectorAll("math")).toHaveLength(400);
    expect(container.querySelectorAll("table")).toHaveLength(200);
    expect(elapsed).toBeLessThan(5000);
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});


test.each(["/*a ", '\\"a '])("highlights 200 KB of unterminated %s without repeated suffix scans", (prefix) => {
  const source = prefix.repeat(Math.ceil(200_000 / prefix.length));
  const start = performance.now();
  const markup = renderToStaticMarkup(<CodeBlock source={source} language="js" />);
  expect(markup).toContain("md-token-");
  expect(markup.length).toBeGreaterThan(source.length);
  expect(performance.now() - start).toBeLessThan(1000);
});
