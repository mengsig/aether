import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, test } from "vitest";
import { FOCUSABLE } from "../src/focus.ts";
import { cssBlocks, type CssBlock } from "./css-blocks.ts";

const read = (name: string): string => readFileSync(path.join(import.meta.dirname, "..", "src", name), "utf8");
const tokens = read("tokens.css");
const base = read("base.css");
const primitives = read("primitives.css");
const shell = read("shell.css");

// CONTRACTS.md §4, name for name.
const COLOUR = ["--color-bg", "--color-surface-1", "--color-surface-2", "--color-surface-3", "--color-border", "--color-border-strong",
  "--color-text", "--color-text-muted", "--color-text-faint", "--color-accent", "--color-accent-contrast", "--color-focus",
  "--color-danger", "--color-warning", "--color-success", "--color-info"];
const STATUS = ["--status-queued", "--status-dispatching", "--status-running", "--status-waiting", "--status-operator",
  "--status-blocked", "--status-succeeded", "--status-failed", "--status-cancelled", "--pr-open", "--pr-draft", "--pr-merged",
  "--pr-closed", "--issue-open", "--issue-closed", "--mastery-unseen", "--mastery-exposed", "--mastery-demonstrated",
  "--mastery-fragile", "--mastery-misconception"];
const TYPE = ["--font-ui", "--font-prose", "--font-mono", "--font-display", "--text-xs", "--text-sm", "--text-md", "--text-lg",
  "--text-xl", "--text-2xl", "--text-3xl", "--leading-tight", "--leading-normal", "--leading-prose", "--weight-regular",
  "--weight-medium", "--weight-bold"];
const SPACE = [...Array.from({ length: 13 }, (_, n) => `--space-${n}`), "--space-px", "--space-0-5", "--blur-sm", "--blur-lg", "--radius-sm", "--radius-md", "--radius-lg", "--radius-xl",
  "--elevation-1", "--elevation-2", "--elevation-3", "--focus-ring"];
const DURATIONS = ["--motion-instant", "--motion-fast", "--motion-base", "--motion-slow"];
const MOTION = [...DURATIONS, "--ease-standard", "--ease-emphasized", "--ease-exit"];
export const CONTRACT_TOKENS = [...COLOUR, ...STATUS, ...TYPE, ...SPACE, ...MOTION];
// Colours and shadows change with the theme; the rest is shared.
const THEMED = [...COLOUR, ...STATUS, "--elevation-1", "--elevation-2", "--elevation-3"];

const blocks = cssBlocks(tokens);
const top = blocks.filter((block) => block.within.length === 0);
const lightBlock = top.find((block) => block.selector.includes('[data-theme="light"]'));
const darkBlock = top.find((block) => block.selector.includes('[data-theme="dark"]'));
const sharedBlock = top.find((block) => block.selector === ":root");
const reducedBlock = blocks.find((block) => block.within.includes("@media (prefers-reduced-motion: reduce)"));

function names(...from: Array<CssBlock | undefined>): Set<string> {
  return new Set(from.flatMap((block) => [...(block?.declarations.keys() ?? [])].filter((name) => name.startsWith("--"))));
}

/** Names missing from a token set; exported so the test of this test can feed it a broken sheet. */
export function missing(required: readonly string[], defined: ReadonlySet<string>): string[] {
  return required.filter((name) => !defined.has(name));
}

/** Custom properties a sheet declares anywhere, in any block or at-rule, that §4 does not name. */
function beyondContract(css: string): string[] {
  return [...names(...cssBlocks(css))].filter((name) => !CONTRACT_TOKENS.includes(name));
}

describe("tokens.css", () => {
  test("defines every §4 name for the light theme", () => {
    expect(lightBlock?.selector).toContain(":root");
    expect(missing(CONTRACT_TOKENS, names(lightBlock, sharedBlock))).toEqual([]);
  });

  test("redefines every colour, status and elevation for the dark theme", () => {
    expect(missing(THEMED, names(darkBlock))).toEqual([]);
    for (const name of THEMED) expect(darkBlock?.declarations.get(name), name).not.toBe(lightBlock?.declarations.get(name));
  });

  test("a missing name fails the check", () => {
    const broken = cssBlocks(tokens.replace(/--status-operator:[^;]*;/, ""));
    const light = broken.find((block) => block.within.length === 0 && block.selector.includes('[data-theme="light"]'));
    expect(missing(CONTRACT_TOKENS, names(light, sharedBlock))).toEqual(["--status-operator"]);
  });

  test("defines nothing beyond §4, in any block", () => {
    expect(blocks.length).toBeGreaterThanOrEqual(4);
    expect(beyondContract(tokens)).toEqual([]);
    const extraRoot = `${tokens}\n:root { --color-uncontracted: #ff0000; }\n`;
    const extraMedia = `${tokens}\n@media (min-width: 1px) { [data-theme="dark"] { --space-13: 52px; } }\n`;
    expect(beyondContract(extraRoot)).toEqual(["--color-uncontracted"]);
    expect(beyondContract(extraMedia)).toEqual(["--space-13"]);
  });

  test("spaces sit on the 4 px grid", () => {
    for (let step = 0; step <= 12; step += 1) expect(sharedBlock?.declarations.get(`--space-${step}`)).toBe(`${step * 4}px`);
  });

  test("every duration token is 0ms under reduced motion, and only there", () => {
    expect(reducedBlock?.selector).toContain(":root");
    for (const name of DURATIONS) {
      expect(reducedBlock?.declarations.get(name), name).toBe("0ms");
      expect(sharedBlock?.declarations.get(name), name).toMatch(/^[1-9]\d*ms$/);
    }
  });

  test("the reduced-motion rule outranks every themed root rule", () => {
    // `:root[data-theme]` matches whenever a theme is set, so a later themed block cannot win.
    expect(reducedBlock?.selector.split(",").map((part) => part.trim())).toEqual([":root", ":root[data-theme]"]);
    expect(names(lightBlock, darkBlock).has("--motion-fast")).toBe(false);
  });
});

describe("the kit's stylesheets", () => {
  const LOCAL = new Set(["--pill-color", "--badge", "--toast-color", "--conn-color"]);
  test.each([["base.css", base], ["primitives.css", primitives], ["shell.css", shell]])("%s reads only defined tokens", (_name, css) => {
    const used = [...css.matchAll(/var\((--[\w-]+)/g)].map((match) => match[1] ?? "");
    expect(used.length).toBeGreaterThan(0);
    expect(used.filter((name) => !CONTRACT_TOKENS.includes(name) && !LOCAL.has(name))).toEqual([]);
  });
});

describe("the focus ring", () => {
  const sheets = [base, primitives, shell].flatMap((css) => cssBlocks(css));

  test("base.css draws --focus-ring on every focusable element, in the last layer", () => {
    expect(base).toMatch(/^@layer ui\.base, ui\.primitives, ui\.shell, ui\.focus;$/m);
    const rule = sheets.find((block) => block.selector === `:where(${FOCUSABLE}):focus-visible`);
    expect(rule?.within).toEqual(["@layer ui.focus"]);
    expect(rule?.declarations.get("box-shadow")).toBe("var(--focus-ring)");
    expect(rule?.declarations.get("outline")).toBe("2px solid transparent");
  });

  test("no rule outside the focus layer styles focus, so none can hide the ring", () => {
    const focusRules = sheets.filter((block) => /:focus/.test(block.selector) && !block.within.includes("@layer ui.focus"));
    const hiding = focusRules.filter((block) => block.declarations.has("box-shadow") || block.declarations.get("outline")?.includes("none"));
    expect(hiding.map((block) => block.selector)).toEqual([]);
  });
});
