// The browser's device key: a non-extractable P-256 key in IndexedDB, keyed by
// origin, in the PWA's database and record shape so old pairings carry over.

import { invariant, object, string } from "../guards.ts";

export interface DeviceIdentity {
  readonly keys: CryptoKeyPair;
  readonly deviceId?: string;
  readonly daemonId?: string;
  readonly name?: string;
  readonly role?: "operator" | "observer";
}

export interface IdentityStore {
  load(): Promise<DeviceIdentity | undefined>;
  save(identity: DeviceIdentity): Promise<void>;
  remove(): Promise<void>;
}

const DB = "aether-remote-device";
const STORE = "identity";

function database(): Promise<IDBDatabase> {
  if (!globalThis.indexedDB) return Promise.reject(new Error("Device pairing requires IndexedDB"));
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
    request.onerror = () => reject(request.error ?? new Error("Could not open device storage"));
    request.onblocked = () => reject(new Error("Device storage upgrade blocked by another tab"));
    request.onsuccess = () => resolve(request.result);
  });
}

async function transact(origin: string, mode: IDBTransactionMode, write: (store: IDBObjectStore) => IDBRequest): Promise<unknown> {
  const db = await database();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE, mode);
    const request = write(tx.objectStore(STORE));
    tx.oncomplete = () => { db.close(); resolve(request.result); };
    tx.onabort = () => { db.close(); reject(tx.error ?? new Error(`Device storage transaction for ${origin} aborted`)); };
    tx.onerror = () => { db.close(); reject(tx.error ?? new Error(`Device storage transaction for ${origin} failed`)); };
  });
}

export function checkIdentity(value: unknown): DeviceIdentity {
  const saved = object(value, "stored device identity");
  const keys = object(saved.keys, "stored device key") as unknown as CryptoKeyPair;
  if (keys.privateKey.extractable || keys.privateKey.algorithm.name !== "ECDSA") throw new Error("Unsafe stored device key");
  if (saved.deviceId !== undefined) string(saved.deviceId, "device id");
  return saved as unknown as DeviceIdentity;
}

export function indexedDbIdentity(origin: string): IdentityStore {
  string(origin, "origin");
  return {
    async load() {
      const saved = await transact(origin, "readonly", (store) => store.get(origin));
      return saved === undefined ? undefined : checkIdentity(saved);
    },
    async save(identity) {
      invariant(!identity.keys.privateKey.extractable, "private keys stay non-extractable");
      string(identity.deviceId, "device id");
      await transact(origin, "readwrite", (store) => store.put(identity, origin));
    },
    async remove() {
      await transact(origin, "readwrite", (store) => store.delete(origin));
    },
  };
}

export async function newIdentity(): Promise<DeviceIdentity> {
  if (!globalThis.crypto?.subtle) throw new Error("Device pairing requires a secure context");
  const keys = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"]);
  invariant(!keys.privateKey.extractable && keys.privateKey.type === "private", "a fresh device key is private and non-extractable");
  return { keys };
}

export function base64url(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

/** The signature over what the remote carrier's challenge binds (aether-remote-auth-v1). */
export async function signChallenge(identity: DeviceIdentity, challenge: { instance: string; nonce: string; origin: string }): Promise<string> {
  const deviceId = string(identity.deviceId, "device id");
  const parts = ["aether-remote-auth-v1", challenge.instance, challenge.origin, deviceId, challenge.nonce];
  if (!/^[A-Za-z0-9_-]{43}$/.test(challenge.nonce) || parts.some((part) => part.includes("\n"))) throw new Error("Invalid authentication challenge");
  invariant(identity.keys.privateKey.type === "private", "the signing key is private");
  const signature = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, identity.keys.privateKey, new TextEncoder().encode(parts.join("\n")));
  return base64url(new Uint8Array(signature));
}
