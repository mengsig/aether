// The packaged-app smoke check (build/smoke.ts sets AETHER_DESKTOP_SMOKE=1): once the app window
// has loaded app://, main prints what the page shows as one JSON line and quits cleanly.
// It imports Electron's types only, so build/smoke.ts can share its names under plain Node.

import type { App } from "electron";
import type { BuildInfo } from "../update/updates.ts";
import type { Desktop } from "./start.ts";

export const SMOKE_ENV = "AETHER_DESKTOP_SMOKE";
export const SMOKE_PREFIX = "AETHER-DESKTOP-SMOKE ";

const PAGE_PROBE = `({
  title: document.title,
  root: document.getElementById("root") !== null,
  bridge: typeof window.desktop === "object" && window.desktop !== null ? window.desktop.version : null,
})`;

export async function reportSmoke(desktop: Desktop, build: BuildInfo, app: Pick<App, "isPackaged" | "quit">): Promise<void> {
  const contents = desktop.appWindow.webContents;
  if (contents.isDestroyed()) throw new Error("the smoke report needs the app window");
  // The document has loaded; its lazy chunks and service worker may still be loading.
  if (contents.isLoading()) await new Promise<void>((resolve) => contents.once("did-stop-loading", () => resolve()));
  const page: unknown = await contents.executeJavaScript(PAGE_PROBE, true);
  const report = { url: contents.getURL(), page, packaged: app.isPackaged, build };
  process.stdout.write(`${SMOKE_PREFIX}${JSON.stringify(report)}\n`);
  app.quit();
}
