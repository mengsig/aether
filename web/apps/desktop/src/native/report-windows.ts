// Report windows: a report page a local server shows (an Archon proposal, a run report) opens in
// a plain window of its own, navigation pinned to that origin, and no popups.

import { BrowserWindow, session, type WebPreferences } from "electron";
import { guardSession, originOf, pinContents } from "../security/navigation.ts";
import { reportPartition, reportWindowOptions } from "./report-window-options.ts";

export class ReportWindows {
  private readonly windows = new Map<string, BrowserWindow>();
  private readonly extra: WebPreferences;

  /** `extra` adds rendering options only, such as offscreen rendering for headless tests. */
  constructor(extra: Pick<WebPreferences, "offscreen"> = {}) {
    this.extra = extra;
  }

  /** The webContents ids of every open report window. */
  contentsIds(): Set<number> {
    return new Set([...this.windows.values()].map((window) => window.webContents.id));
  }

  /** Opens `url` in its own window, or brings that page's window forward. */
  async open(url: string): Promise<BrowserWindow> {
    const origin = originOf(url);
    if (origin === undefined || !url.startsWith(`${origin}/`)) throw new Error(`${url.slice(0, 80)} has an exact origin`);
    const existing = this.windows.get(url);
    if (existing !== undefined && !existing.isDestroyed()) {
      existing.focus();
      return existing;
    }
    const options = reportWindowOptions(origin);
    const window = new BrowserWindow({ ...options, webPreferences: { ...options.webPreferences, ...this.extra } });
    const contents = window.webContents;
    if (contents.session !== session.fromPartition(reportPartition(origin))) throw new Error("a report window uses its origin's partition");
    guardSession(contents.session);
    pinContents(contents, origin);
    window.on("closed", () => this.windows.delete(url));
    this.windows.set(url, window);
    try {
      await window.loadURL(url);
    } catch (error) {
      this.windows.delete(url);
      if (!window.isDestroyed()) window.destroy();
      throw new Error(`The report at ${origin} did not load; is its server still running?`, { cause: error });
    }
    if (this.extra.offscreen !== true) window.show();
    return window;
  }
}
