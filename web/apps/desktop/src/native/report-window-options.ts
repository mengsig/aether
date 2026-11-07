// A report window's options: a plain window for a page a local report server shows. No preload,
// no bridge, and an in-memory partition per origin that is never the app's.

import { createHash } from "node:crypto";
import type { BrowserWindowConstructorOptions } from "electron";
import { originOf } from "../security/navigation.ts";

/** The in-memory partition of one report origin: nothing it stores outlives the app. */
export function reportPartition(origin: string): string {
  if (originOf(`${origin}/`) !== origin || !origin.startsWith("http://")) throw new Error(`${origin} is a loopback http origin`);
  return `report:${createHash("sha256").update(origin).digest("hex")}`;
}

export function reportWindowOptions(origin: string): BrowserWindowConstructorOptions {
  return {
    title: "Report",
    width: 1200,
    height: 860,
    minWidth: 480,
    minHeight: 400,
    show: false,
    autoHideMenuBar: true,
    webPreferences: {
      partition: reportPartition(origin),
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
