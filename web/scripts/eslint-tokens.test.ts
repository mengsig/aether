import path from "node:path";
import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";
import { rawValueProblem, stylesheetProblems } from "./eslint-tokens.ts";

const WEB = path.resolve(import.meta.dirname, "..");
const eslint = new ESLint({ cwd: WEB });
const RULE = "tokens/design-tokens";

/** The design-token messages lint gives a source placed at `file` (relative to web/). */
async function lint(file: string, code: string): Promise<string[]> {
  const [result] = await eslint.lintText(code, { filePath: path.join(WEB, file) });
  expect(result).toBeDefined();
  const messages = result?.messages ?? [];
  expect(messages.filter((message) => message.fatal === true)).toEqual([]);
  return messages.filter((message) => message.ruleId === RULE).map((message) => message.message);
}

const COMPONENT = "packages/ui/src/probe.tsx";

describe("design tokens only (CONTRACTS.md §4)", () => {
  it("rejects raw colours in component sources", async () => {
    const messages = await lint(COMPONENT, [
      'export const a = <span className="x" style={{ color: "#14231f" }} />;',
      'export const b = <svg><path fill="white" /></svg>;',
      'export const c = { background: "oklch(60% 0.1 160)" };',
      "export const d = `0 0 0 1px rgb(0 0 0 / 20%)`;",
    ].join("\n"));
    expect(messages).toHaveLength(4);
    expect(messages[0]).toBe('raw colour "#14231f": use a --color-* token from tokens.css (CONTRACTS.md §4)');
    expect(messages.slice(1).every((message) => message.startsWith("raw colour"))).toBe(true);
  });

  it("rejects raw durations in style objects and animation options", async () => {
    const messages = await lint(COMPONENT, [
      'export const a = <div style={{ transition: "opacity 200ms" }} />;',
      "export const b = <div style={{ animationDuration: 1.5 }} />;",
      "export function c(el: HTMLElement) { el.animate([], { duration: 360 }); }",
      'export function d(el: HTMLElement) { el.style.transition = "transform .2s"; }',
    ].join("\n"));
    expect(messages).toEqual([
      'raw duration "200ms": use a --motion-* token from tokens.css, so reduced motion can zero it (CONTRACTS.md §4)',
      "raw duration 1.5 for animationDuration: use a --motion-* token from tokens.css, so reduced motion can zero it (CONTRACTS.md §4)",
      "raw duration 360 for duration: use a --motion-* token from tokens.css, so reduced motion can zero it (CONTRACTS.md §4)",
      'raw duration ".2s": use a --motion-* token from tokens.css, so reduced motion can zero it (CONTRACTS.md §4)',
    ]);
  });

  it("rejects raw values in reusable style objects, not only inline ones", async () => {
    const messages = await lint(COMPONENT, [
      'const style = { color: "red", backgroundColor: "white", transition: `opacity 200ms` };',
      "export const X = () => <div style={style} />;",
      'export const y = { "--pill-color": "gold", animationDelay: `${1}s, 300ms` };',
    ].join("\n"));
    expect(messages).toEqual([
      'raw colour "red": use a --color-* token from tokens.css (CONTRACTS.md §4)',
      'raw colour "white": use a --color-* token from tokens.css (CONTRACTS.md §4)',
      'raw duration "200ms": use a --motion-* token from tokens.css, so reduced motion can zero it (CONTRACTS.md §4)',
      'raw colour "gold": use a --color-* token from tokens.css (CONTRACTS.md §4)',
      'raw duration "300ms": use a --motion-* token from tokens.css, so reduced motion can zero it (CONTRACTS.md §4)',
    ]);
  });

  it("rejects raw values in every stylesheet a component imports", async () => {
    const messages = await lint(COMPONENT, 'import "../test/fixtures/raw.css";\nexport const x = 1;\n');
    expect(messages).toEqual([
      '../test/fixtures/raw.css:6: raw colour "#00000033": use a --color-* token from tokens.css (CONTRACTS.md §4)',
      '../test/fixtures/raw.css:7: raw duration "150ms": use a --motion-* token from tokens.css, so reduced motion can zero it (CONTRACTS.md §4)',
      '../test/fixtures/raw.css:10: raw colour "white": use a --color-* token from tokens.css (CONTRACTS.md §4)',
    ]);
    const missing = await lint(COMPONENT, 'import "./missing.css";\n');
    expect(missing).toHaveLength(1);
    expect(missing[0]).toMatch(/^cannot read \.\/missing\.css/);
  });

  it("applies to feature packages and the browser entry, not to tests or the token sheet", async () => {
    const raw = 'export const a = "#fff";\n';
    expect(await lint("packages/noesis/src/probe.ts", raw)).toHaveLength(1);
    expect(await lint("apps/web/src/probe.ts", raw)).toHaveLength(1);
    expect(await lint("packages/ui/src/probe.test.ts", raw)).toEqual([]);
    expect(await lint(COMPONENT, 'import "./tokens.css";\nimport "./index.ts";\n')).toEqual([]);
  });

  it("accepts tokens, zero durations and hashes that are not colours", async () => {
    const messages = await lint(COMPONENT, [
      'export const a = <div style={{ color: "var(--color-text)", transition: "opacity var(--motion-fast)" }} />;',
      'export const b = <a href="#/fleet" data-x="#ui-main">Fleet</a>;',
      'export const b2 = <a href="#abc" id="cafe">Fragment</a>;',
      'export const b3 = { label: "Fade", color: "var(--color-text)", transition: `opacity var(--motion-${"fast"})` };',
      'export const c = { transitionDelay: "0ms", duration: 0, label: "Retry in 5s", tone: "red" };',
    ].join("\n"));
    expect(messages).toEqual([]);
  });

  it("reads only declaration values of a sheet", () => {
    expect(stylesheetProblems("a:hover,\n.b:is(.red) { color: var(--c) }\n#abc { --x: 1px }")).toEqual([]);
    expect(stylesheetProblems('.a { background: url("/images/white.png"); font-family: "Tan Sans", serif; content: \'2s\' }')).toEqual([]);
    expect(stylesheetProblems('.a { background: url(x.png) red }')).toEqual([
      '1: raw colour "red": use a --color-* token from tokens.css (CONTRACTS.md §4)',
    ]);
    expect(stylesheetProblems("/* #fff 2s */\n.a { animation: spin 2s linear }")).toEqual([
      '2: raw duration "2s": use a --motion-* token from tokens.css, so reduced motion can zero it (CONTRACTS.md §4)',
    ]);
    expect(stylesheetProblems('@import "./other.css";')[0]).toMatch(/^1: @import hides a sheet/);
    expect(rawValueProblem("calc(var(--motion-slow) * 3)", { colours: true, durations: true, named: true })).toBeUndefined();
    expect(rawValueProblem("hsl(1 2% 3%)", { colours: false, durations: true, named: false })).toBeUndefined();
  });
});
