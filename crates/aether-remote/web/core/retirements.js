import { unconfirmedPrefix } from './unconfirmed.js';
import { string } from './validate.js';
/** @typedef {import('../types.js').Outbox} Outbox */
/** @typedef {'sent'|'failed'|'dismissed'} Retirement */
export const RETIREMENT_LIMIT = 20_000;
/** @param {string} daemonId @param {string} deviceId */
const prefix = (daemonId,deviceId) => `aether.remote.retired.v1:${encodeURIComponent(daemonId)}:${encodeURIComponent(deviceId)}:`;
/** @param {string} daemonId @param {string} deviceId @param {Outbox} row @param {Retirement} status */
function key(daemonId,deviceId,row,status) {
  const id = string(row.id,'receipt ID'); const attempt = string(row.sendId ?? row.attemptId ?? row.id,'receipt attempt');
  return prefix(daemonId,deviceId)+encodeURIComponent(id)+':'+status+(status==='failed' ? ':'+encodeURIComponent(attempt) : '');
}
/** @param {Storage|null} storage @param {string} daemonId @param {string} deviceId @param {Outbox} row @returns {Retirement|null} */
export function readRetirement(storage,daemonId,deviceId,row) {
  for (const status of /** @type {Retirement[]} */ (['dismissed','sent','failed'])) {
    const value = storage?.getItem(key(daemonId,deviceId,row,status));
    if (value == null) continue;
    const decision = status === 'dismissed' ? ['delivered','discarded'].includes(value) : value === 'send_again';
    if (!decision && (!Number.isSafeInteger(Number(value)) || Number(value) < 1)) throw new Error('Invalid receipt retirement marker');
    return status;
  }
  return null;
}
/** @param {Storage} storage @param {string} daemonId @param {string} deviceId @param {Outbox} row @param {Retirement} status @param {'delivered'|'discarded'} [decision] */
export function retire(storage,daemonId,deviceId,row,status,decision) {
  if (!['sent','failed','dismissed'].includes(status)) throw new Error('Cannot retire an unresolved receipt');
  if (decision !== undefined && (status !== 'dismissed' || !['delivered','discarded'].includes(decision))) throw new Error('Invalid delivery decision');
  const marker = key(daemonId,deviceId,row,status);
  if (storage.getItem(marker) != null) { readRetirement(storage,daemonId,deviceId,row); return; }
  storage.setItem(marker,status === 'dismissed' ? decision ?? 'discarded' : row.decision ?? '1');
}
/** @param {Storage} storage @param {string} daemonId @param {string} deviceId @param {number} pending */
export function assertRetirementCapacity(storage,daemonId,deviceId,pending) {
  if (!Number.isSafeInteger(pending) || pending < 0) throw new Error('Invalid pending receipt count');
  const scope = prefix(daemonId,deviceId); const uncertainty = unconfirmedPrefix(daemonId,deviceId); let count = 0;
  for (let index=0;index<storage.length;index++) {
    const name = storage.key(index); if (name?.startsWith(scope) || name?.startsWith(uncertainty)) count++;
  }
  if (count+pending*4 >= RETIREMENT_LIMIT) throw new Error('Delivery history capacity reached. Reconcile pending work, then forget and pair this browser again to clear local history.');
}
/** @param {Outbox} row @param {Retirement} status @returns {Outbox} */
export function retiredRow(row,status) {
  return {...row,status,recovery:undefined,error:status==='failed' ? row.error ?? 'The request was rejected in another tab.' : undefined};
}
