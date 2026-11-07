/// <reference lib="dom" />
// Screenshots of the memory screen over its fixture (packages/aether/src/memory/fixture), at a
// desktop and a narrow phone width. Usage: node dev/memory-capture.ts <out-dir> [prefix] [--flows]

import { mkdirSync } from "node:fs";
import path from "node:path";
import { chromium, type Page } from "@playwright/test";
import { createServer } from "vite";
import { packageAliases, WEB_ROOT } from "../scripts/aliases.ts";

const out = path.resolve(process.argv[2] ?? "memory-capture");
const prefix = process.argv[3] ?? "memory";
const flows = process.argv.includes("--flows");
const VIEWS = { desktop: { width: 1360, height: 900 }, mobile: { width: 390, height: 844 } } as const;
mkdirSync(out, { recursive: true });

const server = await createServer({
  root: path.join(WEB_ROOT, "packages", "aether", "src", "memory", "fixture"),
  resolve: { alias: packageAliases }, server: { port: 0, host: "127.0.0.1" }, logLevel: "warn",
});
await server.listen();
const base = server.resolvedUrls?.local[0];
if (base === undefined) throw new Error("the dev server reported no URL");

async function open(page: Page, query: string): Promise<void> {
  await page.goto(`${base}?${query}`);
  await page.getByRole("heading", { name: "Memory", exact: true }).waitFor();
  await page.locator("article.b3-item").first().waitFor();
}

/** The whole page, or only the section labelled `section`, once its entrance has eased in. */
async function shot(page: Page, name: string, section?: string): Promise<void> {
  await page.waitForTimeout(400);
  const file = path.join(out, `${prefix}-${name}.png`);
  if (section === undefined) {
    await page.screenshot({ path: file, fullPage: true });
  } else {
    // A viewport as tall as the page needs no scroll, so the sticky top bar never covers the section.
    const width = page.viewportSize()?.width ?? VIEWS.desktop.width;
    const height = await page.evaluate(() => document.documentElement.scrollHeight);
    await page.setViewportSize({ width, height });
    await page.locator(`section[aria-label='${section}']`).screenshot({ path: file });
  }
  console.log(`${prefix}-${name}.png`);
}

/** The explain view for the demo task, and the import and consolidate dry runs. */
async function captureFlows(page: Page, view: string): Promise<void> {
  await page.getByLabel("Task id").fill("tsk_01J0G3TASK000000000000001");
  await page.getByRole("button", { name: "Explain", exact: true }).click();
  await page.locator(".mem-turn").first().waitFor();
  await shot(page, `explain-${view}`, "Retrieval explain");
  await page.locator("input[type=file]").setInputFiles({ name: "memories.jsonl", mimeType: "application/jsonl", buffer: Buffer.from('{"format":"aether.memory-export"}\n') });
  await page.getByRole("button", { name: "Dry run import", exact: true }).click();
  await page.locator(".mem-plan[data-kind='import']").waitFor();
  await shot(page, `import-dry-run-${view}`, "Maintenance");
  await page.getByRole("button", { name: "Plan consolidation", exact: true }).click();
  await page.locator(".mem-plan[data-kind='consolidate']").waitFor();
  await shot(page, `consolidate-dry-run-${view}`, "Maintenance");
}

const browser = await chromium.launch();
try {
  for (const [view, viewport] of Object.entries(VIEWS)) {
    const page = await browser.newPage({ viewport });
    await open(page, "theme=light&role=local");
    await page.setViewportSize(viewport);
    await shot(page, `screen-${view}`);
    if (flows) {
      await captureFlows(page, view);
      await page.setViewportSize(viewport);
      await open(page, "theme=dark&role=local");
      await shot(page, `screen-dark-${view}`);
      await open(page, "theme=light&role=observer");
      await shot(page, `observer-${view}`);
      await open(page, "theme=light&role=local&v2=0");
      await shot(page, `older-daemon-${view}`);
    }
    await page.close();
  }
} finally {
  await browser.close();
  await server.close();
}
