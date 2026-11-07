// The HostAdapter of the browser entry (CONTRACTS.md §2.5): a localStorage-backed
// key-value store, and the desktop bridge when Electron's preload provides one.

import type { DesktopBridge, HostAdapter, KeyValueStore } from "@aether-ui/protocol";

const PREFIX = "aether.";

export function localStore(storage: Storage, prefix = PREFIX): KeyValueStore {
  return {
    async get(key) {
      if (key.length === 0) throw new TypeError("a storage key is not empty");
      const raw = storage.getItem(prefix + key);
      if (raw === null) return undefined;
      try {
        return JSON.parse(raw) as unknown;
      } catch (error) {
        throw new Error(`The stored value of ${key} is not JSON`, { cause: error });
      }
    },
    async set(key, value) {
      if (key.length === 0) throw new TypeError("a storage key is not empty");
      if (value === undefined) throw new TypeError(`refusing to store undefined under ${key}`);
      storage.setItem(prefix + key, JSON.stringify(value));
    },
  };
}

/** Electron when its preload exposed bridge version 1, else the browser. */
export function detectHost(scope: { desktop?: unknown; localStorage: Storage }): HostAdapter {
  const storage = localStore(scope.localStorage);
  const bridge = scope.desktop as DesktopBridge | undefined;
  if (bridge === undefined) return { kind: "browser", storage };
  if (bridge.version !== 1) throw new Error(`Unsupported desktop bridge version ${String(bridge.version)}`);
  return { kind: "electron", desktop: bridge, storage };
}
