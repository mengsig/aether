import { object, string } from './validate.js';
/** @typedef {{keys:CryptoKeyPair,deviceId?:string,daemonId?:string,name?:string,role?:'operator'|'observer'}} Identity */
/** @returns {Promise<IDBDatabase>} */
function database() {
  if (!globalThis.indexedDB) return Promise.reject(new Error('Device pairing requires IndexedDB'));
  return new Promise((resolve, reject) => {
    const request = indexedDB.open('aether-remote-device', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('identity');
    request.onerror = () => reject(request.error ?? new Error('Could not open device storage'));
    request.onblocked = () => reject(new Error('Device storage upgrade blocked by another tab'));
    request.onsuccess = () => resolve(request.result);
  });
}
/** @param {'read'|'write'|'delete'} mode @param {Identity} [identity] @returns {Promise<Identity|undefined>} */
async function transaction(mode, identity) {
  const db = await database();
  return new Promise((resolve, reject) => {
    const tx = db.transaction('identity', mode === 'read' ? 'readonly' : 'readwrite');
    const store = tx.objectStore('identity');
    const request = mode === 'read' ? store.get(location.origin) : mode === 'write' ? store.put(identity, location.origin) : store.delete(location.origin);
    tx.oncomplete = () => { db.close(); resolve(request.result); };
    tx.onabort = () => { db.close(); reject(tx.error ?? new Error('Device storage transaction aborted')); };
    tx.onerror = () => { db.close(); reject(tx.error ?? new Error('Device storage transaction failed')); };
  });
}
export async function loadIdentity() {
  const saved = await transaction('read');
  if (!saved) return undefined;
  object(saved.keys, 'stored device key');
  if (saved.keys.privateKey.extractable || saved.keys.privateKey.algorithm.name !== 'ECDSA') throw new Error('Unsafe stored device key');
  return saved;
}
export async function newIdentity() {
  if (!globalThis.crypto?.subtle) throw new Error('Device pairing requires a secure context');
  const keys = await crypto.subtle.generateKey({name:'ECDSA',namedCurve:'P-256'}, false, ['sign','verify']);
  if (keys.privateKey.extractable || keys.privateKey.type !== 'private') throw new Error('Device key generation invariant failed');
  return {keys};
}
/** @param {Identity} identity */
export async function saveIdentity(identity) {
  if (identity.keys.privateKey.extractable) throw new Error('Private keys must be nonextractable');
  string(identity.deviceId, 'device ID');
  if (identity.daemonId !== undefined) string(identity.daemonId, 'daemon ID');
  await transaction('write', identity);
}
export async function deleteIdentity() { await transaction('delete'); }
/** @param {CryptoKey} key @param {string} challenge */
export async function signChallenge(key, challenge) {
  if (key.extractable || key.type !== 'private') throw new Error('Invalid signing key');
  string(challenge, 'challenge');
  const signature = await crypto.subtle.sign({name:'ECDSA',hash:'SHA-256'}, key, new TextEncoder().encode(challenge));
  return base64url(new Uint8Array(signature));
}

/** @param {Uint8Array} bytes */
export function base64url(bytes) {
  return btoa(String.fromCharCode(...bytes)).replaceAll('+','-').replaceAll('/','_').replace(/=+$/,'');
}
