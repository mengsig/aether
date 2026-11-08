// The app window's preload: exposes one frozen object, window.desktop (CONTRACTS.md §2.4),
// whose every method is one IPC call main validates. Sandboxed preloads load as CommonJS.
"use strict";
/* global require, console */

// eslint-disable-next-line @typescript-eslint/no-require-imports
const { contextBridge, ipcRenderer } = require("electron");

/**
 * conn → (callback → how many live subscriptions use it); each frame reaches a callback once.
 * @type {Map<string, Map<(frame: unknown) => void, number>>}
 */
const listeners = new Map();
/** @type {Set<(route: string) => void>} */
const deepLinkListeners = new Set();

ipcRenderer.on("desktop:push", (_event, conn, frame) => {
  for (const callback of listeners.get(conn)?.keys() ?? []) callback(frame);
});
ipcRenderer.on("desktop:deep-link", (_event, route) => {
  for (const callback of deepLinkListeners) callback(route);
});

/**
 * @param {string} method
 * @param {...unknown} args
 * @returns {Promise<any>}
 */
function call(method, ...args) {
  return ipcRenderer.invoke(`desktop:${method}`, ...args);
}

/**
 * @param {string} conn
 * @param {(frame: unknown) => void} callback
 * @returns {() => void}
 */
function hold(conn, callback) {
  const counts = listeners.get(conn) ?? new Map();
  counts.set(callback, (counts.get(callback) ?? 0) + 1);
  listeners.set(conn, counts);
  let held = true;
  return () => {
    if (!held) return;
    held = false;
    const left = (counts.get(callback) ?? 1) - 1;
    if (left > 0) counts.set(callback, left);
    else counts.delete(callback);
    if (counts.size === 0 && listeners.get(conn) === counts) listeners.delete(conn);
  };
}

/** @type {import("@aether-ui/protocol").DesktopBridge & import("@aether-ui/protocol").DesktopSetup} */
const desktop = {
  version: 1,
  discover: () => call("discover"),
  connect: (daemon) => call("connect", daemon),
  request: (conn, frame) => call("request", conn, frame),
  async subscribe(conn, frame, onFrame) {
    if (typeof onFrame !== "function") throw new TypeError("subscribe needs an onFrame callback");
    const release = hold(conn, onFrame);
    try {
      await call("subscribe", conn, frame);
    } catch (error) {
      release();
      throw error;
    }
    return release;
  },
  async close(conn) {
    await call("close", conn);
    listeners.delete(conn);
  },
  privileged: (action) => call("privileged", action),
  startDaemon: (kind) => call("startDaemon", kind),
  pickDirectory: (purpose) => call("pickDirectory", purpose),
  notify(note) {
    call("notify", note).catch((error) => console.error("desktop.notify failed:", error));
  },
  openExternal: (url) => call("openExternal", url),
  openRemote: (origin) => call("openRemote", origin),
  onDeepLink(callback) {
    if (typeof callback !== "function") throw new TypeError("onDeepLink needs a callback");
    deepLinkListeners.add(callback);
    return () => { deepLinkListeners.delete(callback); };
  },
  diagnostics: () => call("diagnostics"),
  getPreference: (key) => call("getPreference", key),
  setPreference: (key, value) => call("setPreference", key, value),
  setup: (step) => call("setup", step),
};

contextBridge.exposeInMainWorld("desktop", Object.freeze(desktop));
