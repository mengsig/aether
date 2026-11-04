// The Electron side of updates and build labelling: electron-updater pinned to this repository's
// releases with every automatic step off, the native confirmations, and the About panel.

import { readFileSync } from "node:fs";
import path from "node:path";
import { app, dialog, type BrowserWindow } from "electron";
import electronUpdater from "electron-updater";
import { confirmNatively } from "../main/host.ts";
import { buildInfo } from "./build-info.ts";
import { ConfirmedStepError, lockDown, UPDATE_FEED, UpdateFlow, updatesSupported, type BuildInfo } from "./updates.ts";

export function readBuildInfo(): BuildInfo {
  const manifest: unknown = JSON.parse(readFileSync(path.join(app.getAppPath(), "package.json"), "utf8"));
  return buildInfo(manifest, app.getVersion(), app.isPackaged);
}

/** The About panel names the build's signing next to its version. */
export function labelAboutPanel(build: BuildInfo): void {
  app.setAboutPanelOptions({ applicationName: "Aether", applicationVersion: build.version, version: build.label });
}

async function reportFailure(window: BrowserWindow, error: unknown): Promise<void> {
  console.error("The update failed:", error);
  if (!(error instanceof ConfirmedStepError) || window.isDestroyed()) return;
  const cause = error.cause instanceof Error ? error.cause.message : String(error.cause);
  await dialog.showMessageBox(window, { type: "error", title: "Update failed", message: error.message, detail: cause, noLink: true });
}

/**
 * Checks this repository's releases once. A download and an install each wait for the user to
 * confirm in a native dialog over `window`; declining either leaves the app as it is.
 */
export function checkForUpdate(window: BrowserWindow, build: BuildInfo): void {
  if (!app.isPackaged || !updatesSupported(process.platform, build)) return;
  const updater = electronUpdater.autoUpdater;
  lockDown(updater);
  updater.setFeedURL({ ...UPDATE_FEED });
  // electron-updater also emits each failure as an event; without a listener Node would throw it.
  updater.on("error", (error) => console.error("The updater reported an error:", error));
  const owner = window.webContents.id;
  const flow = new UpdateFlow(updater, build, (confirmation) => confirmNatively(owner, confirmation));
  flow.run().then(
    (outcome) => console.info(`Update check: ${outcome}`),
    (error: unknown) => reportFailure(window, error).catch((failure: unknown) => console.error("Could not report the update failure:", failure)),
  );
}
