import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
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


test("renders a 200 KB prose message", async () => {
  const prose = "The quick brown fox jumps over the lazy dog while the build runs on every branch and the tests stay green, so nobody **worries** about the `release` until the merge lands.\nA second line follows with more words, see https://example.com/docs for details.\n\n";
  const text = prose.repeat(Math.ceil(200_000 / prose.length));
  let markup = "";
  const elapsed = await timed(async () => { markup = renderToStaticMarkup(<Markdown text={text} />); });
  expect(markup.match(/<a /g)?.length).toBeGreaterThan(500);
  results.time("markdown.prose.ms", elapsed);
});

const hostileFixtures: [string, (scale: number) => string][] = [
  ["unclosed brackets", (n) => "[".repeat(50_000 * n)],
  ["unclosed backticks", (n) => "`a ``b ```c ".repeat(4_000 * n)],
  ["unclosed stars", (n) => "*a ".repeat(16_500 * n)],
  ["unclosed links", (n) => "[a](((".repeat(8_250 * n)],
  ["repeated autolinks", (n) => "http://".repeat(7_000 * n)],
  ["nested emphasis", (n) => "*a ".repeat(12_500 * n) + "a* ".repeat(12_500 * n)],
  ["deep list indent", (n) => `${" ".repeat(1000)}- x\n`.repeat(48 * n)],
];

function cpuTime(text: string): number {
  const start = process.cpuUsage();
  const markup = renderToStaticMarkup(<Markdown text={text} />);
  const used = process.cpuUsage(start);
  expect(markup.length).toBeGreaterThan(0);
  return used.user + used.system;
}

test.each(hostileFixtures)("scales on %s", (_name, fixture) => {
  const small = fixture(1);
  const large = fixture(4);
  cpuTime(small);
  cpuTime(large);
  const ratios: number[] = [];
  for (let i = 0; i < 7; i += 1) {
    const smallCpu = cpuTime(small);
    const largeCpu = cpuTime(large);
    expect(smallCpu).toBeGreaterThan(0);
    ratios.push(largeCpu / smallCpu);
  }
  ratios.sort((a, b) => a - b);
  expect(ratios[3]).toBeLessThan(10);
});
