// The app window: the shared web/dist frontend from app://, with the one preload that exposes
// window.desktop. Every option is the ARCHITECTURE.md §c baseline; the security test pins them.

import type { BrowserWindowConstructorOptions } from "electron";

/** The background the bundle paints first (its theme-color), so launch shows no flash. */
const BACKGROUND = "#0a1311";

export function appWindowOptions(preload: string): BrowserWindowConstructorOptions {
  if (!preload.endsWith("index.cjs")) throw new Error(`the app preload is src/preload/index.cjs, not ${preload}`);
  return {
    title: "Aether",
    width: 1280,
    height: 820,
    minWidth: 720,
    minHeight: 480,
    show: false,
    backgroundColor: BACKGROUND,
    webPreferences: {
      preload,
      backgroundThrottling: false,
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      nodeIntegrationInWorker: false,
      nodeIntegrationInSubFrames: false,
      webSecurity: true,
      allowRunningInsecureContent: false,
      webviewTag: false,
      navigateOnDragDrop: false,
      safeDialogs: true,
      spellcheck: false,
    },
  };
}
