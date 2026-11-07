// A remote window (ARCHITECTURE.md §c): a plain browser on one remote listener's own origin.
// No preload, no bridge, its own partition, navigation pinned, and main holds no IPC channel to it.

import type { BrowserWindowConstructorOptions } from "electron";
import { normalizeOrigin, remotePartition } from "../security/origins.ts";

export function remoteWindowOptions(origin: string): BrowserWindowConstructorOptions {
  if (normalizeOrigin(origin) !== origin) throw new Error(`${origin} is a normalized origin`);
  return {
    title: origin,
    width: 1280,
    height: 820,
    minWidth: 480,
    minHeight: 480,
    show: false,
    webPreferences: {
      partition: remotePartition(origin),
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
