import { expect, test } from "@playwright/test";
import path from "node:path";
import { preview, type PreviewServer } from "vite";

let server: PreviewServer;
let origin: string;

test.beforeAll(async () => {
  server = await preview({
    configFile: path.resolve(import.meta.dirname, "../../../apps/web/vite.config.ts"),
    preview: { host: "127.0.0.1", port: 0 },
  });
  const address = server.httpServer.address();
  if (address === null || typeof address === "string") throw new Error("Vite preview did not bind a TCP port");
  origin = `http://127.0.0.1:${address.port}`;
});

test.afterAll(async () => {
  await new Promise<void>((resolve, reject) => server.httpServer.close((error) => error ? reject(error) : resolve()));
});

test.use({ serviceWorkers: "block" });

test("the palette reports a failed chunk in a visible overlay and retries", async ({ page }) => {
  let failedPath = "";
  let failedChunkRequests = 0;
  await page.route(/\/assets\/(?:command-palette|entity-palette)-[^/]+\.js(?:\?.*)?$/, async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (failedPath === "") {
      failedPath = path;
      failedChunkRequests += 1;
      await route.abort("failed");
      return;
    }
    if (path === failedPath) failedChunkRequests += 1;
    await route.continue();
  });

  await page.goto(origin);
  await expect(page.locator(".ui-navrail")).toBeVisible();
  await page.keyboard.press("Control+k");

  const failure = page.locator(".ui-palette-failure[role=alert]");
  await expect(failure).toBeVisible();
  await expect(failure).toBeInViewport({ ratio: 1 });
  await expect(failure).toHaveCSS("position", "fixed");
  await expect(failure).toHaveCSS("z-index", "55");
  await expect(failure).toContainText("The palette failed");
  expect(failedChunkRequests).toBe(1);

  await failure.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByRole("dialog", { name: "Go anywhere" })).toBeVisible();
  await expect(failure).toHaveCount(0);
  expect(failedChunkRequests).toBeGreaterThanOrEqual(2);
});
