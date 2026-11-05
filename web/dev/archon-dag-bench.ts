/// <reference lib="dom" />
// Measures the Archon task graph's pan and zoom in headless Chromium: `node dev/archon-dag-bench.ts [out.json]`.
// It serves the C2 fixture with Vite, opens 2,000 items over the faux archond, and drives real wheel and
// drag input one step per frame. It reports each rAF callback's time (the renderer draws in its rAF) and
// the intervals between frames, against the 16.7 ms budget of 60 fps.

import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import path from "node:path";
import { chromium, type Page } from "@playwright/test";
import { createServer } from "vite";
import { packageAliases, WEB_ROOT } from "../scripts/aliases.ts";

const BUDGET_MS = 1000 / 60;
const COUNT = 2000;
const VIEW = { width: 1440, height: 900 };

interface Recorded { callbacks: number[]; frames: number[] }
interface Stats { count: number; p50: number; p95: number; max: number }

/** Times every rAF callback the page schedules, and keeps a frame clock of its own. */
function instrument(): void {
  const raf = window.requestAnimationFrame.bind(window);
  const record: Recorded & { on: boolean } = { callbacks: [], frames: [], on: false };
  Object.assign(window, { benchRecord: record, benchFrame: raf });
  window.requestAnimationFrame = (callback) => raf((time) => {
    const started = performance.now();
    callback(time);
    if (record.on) record.callbacks.push(performance.now() - started);
  });
  const clock = (time: number) => {
    if (record.on) record.frames.push(time);
    raf(clock);
  };
  raf(clock);
}

function stats(values: readonly number[]): Stats {
  assert.ok(values.length > 0, "a measurement has samples");
  const sorted = [...values].sort((a, b) => a - b);
  const at = (p: number) => sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] ?? NaN;
  return { count: sorted.length, p50: at(0.5), p95: at(0.95), max: sorted.at(-1) ?? NaN };
}

/** Waits a frame on the unwrapped rAF, so the wait is not counted as the page's work. */
const nextFrame = (page: Page) => page.evaluate(() => new Promise<void>((resolve) => {
  (window as unknown as { benchFrame: typeof requestAnimationFrame }).benchFrame(() => resolve());
}));

/** Zooms in, drags across and zooms out, one input step per frame. */
async function drive(page: Page, centre: { x: number; y: number }): Promise<number> {
  let steps = 0;
  await page.mouse.move(centre.x, centre.y);
  for (let i = 0; i < 90; i++, steps++) { await page.mouse.wheel(0, -40); await nextFrame(page); }
  await page.mouse.down();
  for (let i = 0; i < 180; i++, steps++) {
    await page.mouse.move(centre.x + (i < 90 ? -i : i - 180) * 6, centre.y + (i % 20) * 3);
    await nextFrame(page);
  }
  await page.mouse.up();
  for (let i = 0; i < 90; i++, steps++) { await page.mouse.wheel(0, 40); await nextFrame(page); }
  return steps;
}

async function measure(url: string): Promise<Record<string, unknown>> {
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: VIEW, deviceScaleFactor: 1 });
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.addInitScript(instrument);
    await page.goto(url);
    await page.waitForSelector(".dag-canvas canvas", { state: "attached", timeout: 60_000 });
    await page.waitForFunction(() => document.querySelector(".dag-placing") === null && document.querySelector(".dag-canvas[aria-busy='true']") === null, null, { timeout: 30_000 });
    const items = await page.locator("ul[aria-label^='Task graph'] li").count();
    const box = await page.locator(".dag-canvas canvas").boundingBox();
    assert.ok(box !== null, "the canvas has a box");
    await page.evaluate(() => { (window as unknown as { benchRecord: { on: boolean } }).benchRecord.on = true; });
    const started = Date.now();
    const steps = await drive(page, { x: box.x + box.width / 2, y: box.y + box.height / 2 });
    const elapsed = Date.now() - started;
    const record = await page.evaluate(() => (window as unknown as { benchRecord: Recorded }).benchRecord);
    assert.deepEqual(errors, [], "the page raised no errors");
    const intervals = record.frames.slice(1).map((time, i) => time - (record.frames[i] ?? time));
    const draw = stats(record.callbacks);
    const interval = stats(intervals);
    const late = intervals.filter((gap) => gap > BUDGET_MS * 1.5).length;
    return { url, items, steps, elapsedMs: elapsed, budgetMs: BUDGET_MS, rafCallbackMs: draw, frameIntervalMs: interval, framesOver25ms: late, framesRecorded: record.frames.length };
  } finally {
    await browser.close();
  }
}

const out = process.argv[2];
const root = path.join(WEB_ROOT, "packages/archon/src/dag/fixture");
const server = await createServer({ root, configFile: false, logLevel: "warn", resolve: { alias: packageAliases }, server: { host: "127.0.0.1", port: 5240, fs: { allow: [WEB_ROOT] } } });
try {
  await server.listen();
  const address = server.httpServer?.address();
  assert.ok(address !== null && typeof address === "object", "vite listens on a port");
  const result = await measure(`http://127.0.0.1:${address.port}/?world=large&count=${COUNT}#/archon/graph`);
  console.log(JSON.stringify(result, null, 2));
  if (out !== undefined) await writeFile(out, `${JSON.stringify(result, null, 2)}\n`);
  const draw = result.rafCallbackMs as Stats;
  assert.ok(draw.p95 < BUDGET_MS, `p95 rAF callback ${draw.p95.toFixed(2)} ms is within the ${BUDGET_MS.toFixed(1)} ms budget`);
} finally {
  await server.close();
}
