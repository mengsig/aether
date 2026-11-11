import { assertSubmissionLock } from './submission-locks.js';
import { object, string } from './validate.js';
import { readLedger, writeLedger, interruptedReceipts } from './ledger.js';
/** @param {Storage|null} storage */
export function createDrafts(storage) {
  /** @type {Map<string,string>} */ const drafts = new Map();
  if (storage) {
    const raw = storage.getItem('aether.remote.drafts.v1');
    if (raw) {
      if (raw.length > 200_000) throw new Error('Saved drafts exceed the storage limit');
      for (const [id,text] of Object.entries(object(JSON.parse(raw)))) {
        string(id); if (typeof text !== 'string' || text.length > 20_000) throw new Error('Invalid saved draft');
        drafts.set(id,text);
      }
      if (drafts.size > 50) throw new Error('Too many saved drafts');
    }
  }
  return {
    /** @param {string} id */
    get: id => drafts.get(id) ?? '',
    /** @param {string} id @param {string} text */
    set(id, text) {
      string(id, 'draft agent');
      if (typeof text !== 'string' || text.length > 20_000) throw new Error('Drafts are limited to 20,000 characters');
      const next = new Map(drafts); next.delete(id); if (text) next.set(id,text);
      while (next.size > 50) next.delete(/** @type {string} */ (next.keys().next().value));
      let encoded = JSON.stringify(Object.fromEntries(next));
      while (encoded.length > 200_000 && next.size > 1) {
        next.delete(/** @type {string} */ (next.keys().next().value)); encoded = JSON.stringify(Object.fromEntries(next));
      }
      storage?.setItem('aether.remote.drafts.v1',encoded);
      drafts.clear(); for (const [key,value] of next) drafts.set(key,value);
    },
  };
}
/** @param {Storage|null} storage @returns {Partial<import('../types.js').State> & {deviceId?:string}} */
export function restoreSnapshot(storage) {
  const encoded = storage?.getItem('aether.remote.state.v1');
  if (!encoded) return {};
  if (encoded.length > 2_000_000) throw new Error('Saved remote state exceeds the storage limit');
  const value = object(JSON.parse(encoded));
  if (value.v !== 2 || typeof value.lastSeq !== 'number' || !Number.isSafeInteger(value.lastSeq) || value.lastSeq < 0) throw new Error('Invalid saved cursor');
  const daemonId = string(value.daemonId);
  const activity = snapshotArray(value.activity,500); const attention = snapshotArray(value.attention,1000);
  for (const row of activity) { string(row.kind); if (!Number.isSafeInteger(row.seq)) throw new Error('Invalid saved activity'); }
  for (const row of attention) { string(row.id); string(row.agentId); string(row.kind); }
  const deviceId = typeof value.deviceId === 'string' ? value.deviceId : '';
  const outbox = interruptedReceipts(readLedger(storage,daemonId,deviceId));
  return /** @type {Partial<import('../types.js').State> & {deviceId:string}} */ ({daemonId,deviceId,lastSeq:value.lastSeq,activity,attention,outbox});
}
/** @param {unknown} value @param {number} limit */
function snapshotArray(value, limit) {
  if (!Array.isArray(value) || value.length > limit) throw new Error('Invalid saved state collection');
  return value.map(item => object(item));
}
/** @param {Storage|null} storage @param {import('../types.js').State} state @param {import('../types.js').State} previous @param {string} deviceId @param {import('./submission-locks.js').SubmissionLock} [lease] */
export function saveSnapshot(storage, state, previous, deviceId,lease) {
  if (!storage || !state.daemonId) return state;
  if (!Number.isSafeInteger(state.lastSeq)) throw new Error('Invalid state persistence bounds');
  const value = {v:2,daemonId:state.daemonId,deviceId,lastSeq:state.lastSeq,activity:state.activity,attention:state.attention};
  const encoded = JSON.stringify(value);
  if (encoded.length > 2_000_000) throw new Error('Remote state storage limit reached; settle or clear old receipts before sending');
  storage.setItem('aether.remote.state.v1',encoded);
  if (!lease || state.outbox === previous.outbox) return state;
  assertSubmissionLock(lease,{daemonId:state.daemonId,deviceId},lease.id);
  const changed = writeLedger(storage,state.daemonId,deviceId,state.outbox.filter(row=>row.id===lease.id),previous.outbox.filter(row=>row.id===lease.id));
  const outbox = state.outbox.map(row=>row.id===lease.id ? changed[0] : row);
  return {...state,outbox};
}
