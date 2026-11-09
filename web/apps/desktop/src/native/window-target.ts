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

export function x11Session(platform: string, ozone: string, hint: string, env: Readonly<Record<string, string | undefined>>): boolean {
  if (platform !== "linux") return false;
  const selected = ozone !== "" && ozone !== "auto" ? ozone : hint;
  if (selected !== "" && selected !== "auto") return selected === "x11";
  if (env.XDG_SESSION_TYPE === "x11") return true;
  if (env.XDG_SESSION_TYPE === "wayland") return false;
  return Boolean(env.DISPLAY) && !env.WAYLAND_DISPLAY;
}

/** Restores the app and delivers routes to its renderer. */
export function windowTarget(window: AppWindow, remapX11: boolean): RouteTarget {
  const focus = () => {
    if (window.isDestroyed()) throw new Error("the app window is gone");
    const minimized = window.isMinimized();
    if (minimized) window.restore();
    // X11 can unmap a window while Electron still considers it visible.
    else if (remapX11 && window.isVisible()) window.hide();
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
