// Opens the gallery in headless Chromium: screenshots in both themes, axe with every
// rule (contrast included), the focus ring on each Tab stop, reduced motion and the
// virtual list's DOM size. Usage: node apps/web/scripts/capture.ts <out-dir>

import { mkdirSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { chromium, type Page } from "@playwright/test";
import { createServer } from "vite";

const out = path.resolve(process.argv[2] ?? "capture");
mkdirSync(out, { recursive: true });
const axePath = createRequire(import.meta.url).resolve("axe-core/axe.min.js");
const server = await createServer({ configFile: path.join(import.meta.dirname, "..", "vite.config.ts"), server: { port: 0, host: "127.0.0.1" }, logLevel: "warn" });
await server.listen();
const base = server.resolvedUrls?.local[0];
if (base === undefined) throw new Error("the dev server reported no URL");

async function axe(page: Page): Promise<string[]> {
  await page.addScriptTag({ path: axePath });
  return page.evaluate(async () => {
    const result = await (window as unknown as { axe: { run(): Promise<{ violations: Array<{ id: string; nodes: unknown[] }> }> } }).axe.run();
    return result.violations.map((violation) => `${violation.id} (${violation.nodes.length})`);
  });
}

/** WCAG 1.4.13 in a real browser: a hover tooltip survives the pointer moving onto it, and Escape hides it. */
async function tooltipProbe(page: Page): Promise<Record<string, boolean>> {
  const anchor = page.locator(".ui-tooltip-anchor").first();
  const tip = anchor.locator("[role=tooltip]");
  const isOpen = async () => (await tip.getAttribute("data-open")) === "true";
  await anchor.locator("button").hover();
  const onHover = await isOpen();
  const box = await tip.boundingBox();
  if (box === null) throw new Error("the tooltip has no box");
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 8 });
  const onTooltip = await isOpen();
  await page.keyboard.press("Escape");
  const afterEscape = await isOpen();
  await page.mouse.move(0, 0);
  return { onHover, onTooltip, afterEscape };
}

async function tabStops(page: Page, limit: number): Promise<Array<{ element: string; ring: boolean }>> {
  const stops: Array<{ element: string; ring: boolean }> = [];
  for (let step = 0; step < limit; step += 1) {
    await page.keyboard.press("Tab");
    stops.push(await page.evaluate(() => {
      const element = document.activeElement as HTMLElement;
      // Tab past the last stop moves focus to the browser itself, leaving the body active.
      if (element === document.body) return { element: "(focus left the page)", ring: true };
      const style = getComputedStyle(element);
      const label = `${element.tagName.toLowerCase()}.${element.className.split(" ")[0] ?? ""} ${(element.textContent ?? "").trim().slice(0, 24)}`;
      return { element: label, ring: style.boxShadow !== "none" || style.outlineStyle !== "none" };
    }));
  }
  return stops;
}

const report: Record<string, unknown> = {};
const browser = await chromium.launch();
try {
  for (const theme of ["light", "dark"] as const) {
    const page = await browser.newPage({ viewport: { width: 1360, height: 900 }, colorScheme: theme });
    await page.goto(`${base}gallery.html?theme=${theme}#/fleet`);
    await page.locator(".ui-vlist-row").first().waitFor();
    await page.screenshot({ path: path.join(out, `shell-${theme}.png`) });
    report[`axe-${theme}`] = await axe(page);
    await page.close();
  }
  const page = await browser.newPage({ viewport: { width: 1360, height: 900 } });
  await page.goto(`${base}gallery.html?theme=light#/fleet`);
  await page.locator(".ui-vlist-row").first().waitFor();
  // The gallery's rows hold several elements each, so the row count is the measure here.
  report.vlist = await page.evaluate(async () => {
    const list = document.querySelector(".ui-vlist");
    if (!list) throw new Error("no list");
    list.scrollTop = 200_000;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const rows = list.querySelectorAll(".ui-vlist-row");
    return { rows: rows.length, nodes: list.querySelectorAll("*").length + 1, setSize: rows[0]?.getAttribute("aria-setsize") };
  });
  report.tooltip = await tooltipProbe(page);
  report.tabStops = await tabStops(page, 30);
  await page.emulateMedia({ reducedMotion: "reduce" });
  report.reducedMotion = await page.evaluate(() => ["instant", "fast", "base", "slow"].map((name) => getComputedStyle(document.documentElement).getPropertyValue(`--motion-${name}`).trim()));
  await page.goto(`${base}gallery.html?theme=dark#/archon/queue`);
  await page.locator(".ui-state").waitFor();
  await page.screenshot({ path: path.join(out, "shell-dark-error.png") });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${base}gallery.html?theme=light#/fleet`);
  await page.locator(".ui-vlist-row").first().waitFor();
  await page.screenshot({ path: path.join(out, "shell-light-narrow.png") });
  report["axe-narrow"] = await axe(page);
} finally {
  await browser.close();
  await server.close();
}
writeFileSync(path.join(out, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));
