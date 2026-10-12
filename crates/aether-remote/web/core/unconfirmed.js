import { object, string } from './validate.js';
/** @typedef {import('../types.js').Outbox} Outbox */
/** @typedef {{reason:string,message:string}} Unconfirmed */
/** @param {string} daemonId @param {string} deviceId */
export const unconfirmedPrefix = (daemonId,deviceId) => `aether.remote.unconfirmed.v1:${encodeURIComponent(daemonId)}:${encodeURIComponent(deviceId)}:`;
/** @param {unknown} value @returns {Unconfirmed} */
export function validateUnconfirmed(value) {
  const data = object(value,'unconfirmed delivery');
  return {reason:string(data.reason,'unconfirmed reason'),message:string(data.message,'unconfirmed explanation')};
}
/** @param {Storage|null} storage @param {string} daemonId @param {string} deviceId @param {string} id */
export function readUnconfirmed(storage,daemonId,deviceId,id) {
  const key = unconfirmedPrefix(daemonId,deviceId)+encodeURIComponent(string(id,'receipt ID'));
  const raw = storage?.getItem(key);
  return raw == null ? null : validateUnconfirmed(JSON.parse(raw));
}
/** @param {Storage|null} storage @param {string} daemonId @param {string} deviceId @param {string} id @param {Unconfirmed} detail */
export function markUnconfirmed(storage,daemonId,deviceId,id,detail) {
  const value = readUnconfirmed(storage,daemonId,deviceId,id) ?? validateUnconfirmed(detail);
  if (storage && !daemonId) throw new Error('Unconfirmed delivery has no daemon identity');
  storage?.setItem(unconfirmedPrefix(daemonId,deviceId)+encodeURIComponent(id),JSON.stringify(value));
  return value;
}
/** @param {Storage|null} storage @param {string} daemonId @param {string} deviceId @param {Outbox} row @returns {Outbox} */
export function withUnconfirmed(storage,daemonId,deviceId,row) {
  const held = row.unconfirmed == null ? null : validateUnconfirmed(row.unconfirmed);
  const unconfirmed = readUnconfirmed(storage,daemonId,deviceId,row.id) ?? held;
  return unconfirmed ? {...row,unconfirmed} : row;
}
