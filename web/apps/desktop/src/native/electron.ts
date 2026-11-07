// The Electron side of the native surfaces: the notification sink, the folder picker, the app
// window as a deep-link target, and the link scheme registration. Wording stays short and calm.

import { app, BrowserWindow, dialog, Notification, webContents } from "electron";
import { DEEP_LINK_CHANNEL, DEEP_LINK_SCHEME, type RouteTarget } from "./deep-links.ts";
import { checkPurpose, type FolderTokens } from "./folder-tokens.ts";
import type { NotificationSink } from "./notifications.ts";

const MAX_SHOWN = 32;

/** Native notifications; each is held until it closes, so its click still reaches main. */
export function notificationSink(): NotificationSink {
  const shown = new Set<Notification>();
  return {
    show(content, onClick) {
      if (!Notification.isSupported()) throw new Error("This system shows no notifications");
      const note = new Notification({ title: content.title, body: content.body, silent: true });
      note.on("click", onClick);
      note.on("close", () => shown.delete(note));
      if (shown.size >= MAX_SHOWN) shown.delete(shown.values().next().value as Notification);
      shown.add(note);
      note.show();
    },
  };
}

/** The app window, once its first load finished, as the place links and notification clicks open. */
export function windowTarget(window: BrowserWindow): RouteTarget {
  const focus = () => {
    if (window.isDestroyed()) throw new Error("the app window is gone");
    if (window.isMinimized()) window.restore();
    window.focus();
  };
  return {
    isReady: () => !window.isDestroyed() && !window.webContents.isCrashed(),
    focus,
    show(route) {
      focus();
      window.webContents.send(DEEP_LINK_CHANNEL, route);
    },
  };
}

/** The native folder picker over the asking window; a token for the choice, or null if cancelled. */
export async function pickFolder(owner: number, purpose: string, tokens: FolderTokens): Promise<string | null> {
  checkPurpose(purpose);
  const contents = webContents.fromId(owner);
  const window = contents === undefined ? null : BrowserWindow.fromWebContents(contents);
  if (window === null) throw new Error("The window that asked for a folder is gone");
  const picked = await dialog.showOpenDialog(window, {
    title: "Choose a folder",
    buttonLabel: "Choose",
    properties: ["openDirectory", "createDirectory", "dontAddToRecent"],
  });
  const folder = picked.filePaths[0];
  if (picked.canceled || folder === undefined) return null;
  if (picked.filePaths.length !== 1) throw new Error("the picker returns one folder");
  return tokens.issue(folder, purpose);
}

/**
 * Makes the system send `aether-app://` links to this app. Only an installed build registers:
 * a development run must not take the scheme over on the machine it runs on.
 */
export function registerDeepLinkScheme(): void {
  if (!app.isPackaged) return;
  if (!app.setAsDefaultProtocolClient(DEEP_LINK_SCHEME)) console.warn(`Could not register ${DEEP_LINK_SCHEME}:// links`);
}
