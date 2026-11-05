// Motion runs on the kit's duration tokens alone: `prefers-reduced-motion: reduce` zeroes every
// animation and transition, and without it nothing a person waits on runs longer than a beat.

import assert from "node:assert/strict";
import { pairBoth } from "../../harness/pair.ts";
import { expect, test, type Page } from "../../harness/fixtures.ts";

/** The longest a finite animation or a transition may take; looping indicators are exempt. */
const LONGEST_S = 0.3;

interface Motion {
  readonly element: string;
  readonly animation: string;
  readonly animationS: number;
  readonly loops: boolean;
  readonly transitionS: number;
}

/** Every element or pseudo-element with a non-zero animation or transition, as the browser computed it. */
async function motions(page: Page): Promise<Motion[]> {
  return page.evaluate(() => {
    const seconds = (list: string): number => Math.max(...list.split(",").map((part) => {
      const value = part.trim();
      return value.endsWith("ms") ? Number.parseFloat(value) / 1000 : Number.parseFloat(value);
    }));
    const found: Motion[] = [];
    for (const element of document.querySelectorAll("body *")) {
      for (const pseudo of [undefined, "::before", "::after"]) {
        const style = getComputedStyle(element, pseudo);
        if (pseudo !== undefined && style.content === "none") continue;
        const animationS = style.animationName === "none" ? 0 : seconds(style.animationDuration);
        const transitionS = seconds(style.transitionDuration);
        if (animationS === 0 && transitionS === 0) continue;
        const name = `${element.tagName.toLowerCase()}.${element.className.toString().split(" ")[0] ?? ""}${pseudo ?? ""}`;
        found.push({ element: name, animation: style.animationName, animationS, loops: style.animationIterationCount === "infinite", transitionS });
      }
    }
    return found;
  });
}

async function open(page: Page, origin: string, hash: string): Promise<void> {
  assert.ok(hash.startsWith("#/"), `a route: ${hash}`);
  await page.goto(`${origin}/${hash}`);
  await expect(page.locator(".ui-screen-skeleton")).toHaveCount(0);
  await expect(page.locator(".ui-navrail")).toBeVisible();
}

const ROUTES = ["#/fleet", "#/pair/aether"];

test("reduced motion turns every animation and transition off", async ({ page, stack }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  for (const hash of ROUTES) {
    await open(page, stack.aether.origin, hash);
    expect(await motions(page), `${hash} under prefers-reduced-motion`).toEqual([]);
  }
  await pairBoth(page, stack);
  await open(page, stack.aether.origin, "#/archon/queue");
  await expect(page.locator(".queue-pause")).toBeVisible();
  expect(await motions(page), "the queue under prefers-reduced-motion").toEqual([]);
});

test("with motion on, the kit animates and nothing finite outlasts a beat", async ({ page, stack }) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await pairBoth(page, stack);
  await open(page, stack.aether.origin, "#/archon/queue");
  await expect(page.locator(".queue-pause")).toBeVisible();
  const found = await motions(page);
  const slow = found.filter((motion) => (motion.animationS > LONGEST_S && !motion.loops) || motion.transitionS > LONGEST_S);
  expect(slow, `nothing finite runs longer than ${LONGEST_S}s`).toEqual([]);
  // The documented pieces: a page that rises in, buttons and nav links that respond; anything that loops is an indicator.
  const byName = (name: string) => found.filter((motion) => motion.animation === name);
  expect(byName("ui-rise").length, "a screen rises in").toBeGreaterThan(0);
  expect(found.some((motion) => motion.element.startsWith("button.ui-button") && motion.transitionS > 0), "buttons transition").toBe(true);
  expect(found.some((motion) => motion.element === "a.ui-navrail-link" && motion.transitionS > 0), "nav links transition").toBe(true);
  for (const motion of found.filter((entry) => entry.loops)) {
    expect(["ui-pulse", "ui-shimmer", "learn-breathe"], `${motion.element} loops ${motion.animation}`).toContain(motion.animation);
  }
});
