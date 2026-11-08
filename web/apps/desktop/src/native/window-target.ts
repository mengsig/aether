import { DEEP_LINK_CHANNEL, type RouteTarget } from "./deep-links.ts";

interface AppWindow {
  isDestroyed(): boolean;
  isMinimized(): boolean;
  isVisible(): boolean;
  restore(): void;
  hide(): void;
  show(): void;
  focus(): void;
  webContents: { isCrashed(): boolean; send(channel: string, route: string): void };
}

/** Restores the app and delivers routes to its renderer. */
export function windowTarget(window: AppWindow): RouteTarget {
  const focus = () => {
    if (window.isDestroyed()) throw new Error("the app window is gone");
    if (window.isMinimized()) window.restore();
    // X11 can unmap a window while Electron still considers it visible.
    if (window.isVisible()) window.hide();
    window.show();
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
