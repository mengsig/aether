// Opens and forgets remote windows. Each loads its origin's own listener, which serves the same
// web/dist; main never injects anything into it, and a removed origin's partition is cleared.

import { BrowserWindow, session, type WebPreferences } from "electron";
import { guardSession, pinContents } from "../security/navigation.ts";
import { remotePartition } from "../security/origins.ts";
import { remoteWindowOptions } from "./remote-window.ts";

export class RemoteWindows {
  private readonly windows = new Map<string, BrowserWindow>();
  private readonly extra: WebPreferences;

  /** `extra` adds rendering options only, such as offscreen rendering for headless tests. */
  constructor(extra: Pick<WebPreferences, "offscreen"> = {}) {
    this.extra = extra;
  }

  /** The webContents ids of every open remote window. */
  contentsIds(): Set<number> {
    return new Set([...this.windows.values()].map((window) => window.webContents.id));
  }

  window(origin: string): BrowserWindow | undefined {
    return this.windows.get(origin);
  }

  async open(origin: string): Promise<BrowserWindow> {
    const existing = this.windows.get(origin);
    if (existing !== undefined && !existing.isDestroyed()) {
      existing.focus();
      return existing;
    }
    const options = remoteWindowOptions(origin);
    const window = new BrowserWindow({ ...options, webPreferences: { ...options.webPreferences, ...this.extra } });
    const contents = window.webContents;
    if (contents.session !== session.fromPartition(remotePartition(origin))) throw new Error("a remote window uses its origin's partition");
    guardSession(contents.session);
    pinContents(contents, origin);
    window.on("page-title-updated", (event) => event.preventDefault());
    window.on("closed", () => this.windows.delete(origin));
    this.windows.set(origin, window);
    try {
      await window.loadURL(`${origin}/`);
    } catch (error) {
      // A window whose first load failed is never reused: the next open tries the origin again.
      this.windows.delete(origin);
      if (!window.isDestroyed()) window.destroy();
      throw new Error(`Could not open ${origin}`, { cause: error });
    }
    if (this.extra.offscreen !== true) window.show();
    return window;
  }

  /** Closes the origin's window and clears everything its partition stored. */
  async forget(origin: string): Promise<void> {
    const window = this.windows.get(origin);
    if (window !== undefined && !window.isDestroyed()) window.destroy();
    this.windows.delete(origin);
    await session.fromPartition(remotePartition(origin)).clearStorageData();
  }
}
