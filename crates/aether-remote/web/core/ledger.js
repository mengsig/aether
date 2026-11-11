import { withUnconfirmed } from './unconfirmed.js';
import { assertRetirementCapacity, readRetirement, retire, retiredRow } from './retirements.js';
import { integer, object, string } from './validate.js';
/** @typedef {import('../types.js').Outbox} Outbox */
/** @param {string} daemonId @param {string} deviceId */
const prefix = (daemonId,deviceId) => `aether.remote.receipt.v1:${encodeURIComponent(daemonId)}:${encodeURIComponent(deviceId)}:`;

/** @param {string} daemonId @param {string} deviceId @param {Outbox} row */
const receiptKey = (daemonId,deviceId,row) => prefix(daemonId,deviceId)+encodeURIComponent(row.id)+(row.sendId || row.attemptId ? ':'+encodeURIComponent(row.sendId ?? row.attemptId ?? '') : '');

/** @param {Storage|null} storage @param {string} daemonId @param {string} deviceId @returns {Outbox[]} */
function readAttempts(storage, daemonId, deviceId) {
  if (!storage || !daemonId) return [];
  const scope = prefix(daemonId,deviceId); const rows = [];
  for (let index = 0; index < storage.length; index++) {
    const key = storage.key(index); if (!key?.startsWith(scope)) continue;
    const raw = storage.getItem(key); if (!raw) continue;
    if (raw.length > 2_000_000 || rows.length >= 1000) throw new Error('Receipt ledger exceeds its storage limit');
    const row = object(JSON.parse(raw)); string(row.id); string(object(row.command).cmd);
    if (key !== receiptKey(daemonId,deviceId,/** @type {Outbox} */ (row))) throw new Error('Receipt storage key differs from its identity');
    if (!['pending','sent','failed','unknown'].includes(String(row.status))) throw new Error('Invalid saved receipt');
    if (typeof row.agentId !== 'string' || typeof row.text !== 'string') throw new Error('Invalid receipt display fields');
    if (row.retentionRisk != null && typeof row.retentionRisk !== 'string') throw new Error('Invalid receipt retention warning');
    if (row.recovery != null && !['resolving','unsupported','unknown','error'].includes(String(row.recovery))) throw new Error('Invalid saved recovery state');
    if (row.decision != null && row.decision !== 'send_again') throw new Error('Invalid saved delivery decision');
    if (row.reply != null) string(object(row.reply).reply,'saved reply tag');
    if (row.horizon != null) string(row.horizon,'saved submission horizon');
    if (row.createdOrder != null) integer(row.createdOrder,'receipt creation order');
    if (row.command && object(row.command).idempotency_key != null && object(row.command).idempotency_key !== row.id) throw new Error('Saved receipt key differs from command');
    if (row.attemptId != null) string(row.attemptId,'saved attempt identity');
    if (row.sendId != null) string(row.sendId,'saved wire identity');
    if (row.wireOrder != null) integer(row.wireOrder,'saved wire order');
    if (!readRetirement(storage,daemonId,deviceId,/** @type {Outbox} */ (row))) rows.push(withUnconfirmed(storage,daemonId,deviceId,/** @type {Outbox} */ (row)));
  }
  return rows;
}
/** @param {Outbox[]} rows */
function latestReceipts(rows) {
  /** @type {Map<string,Outbox>} */ const latest = new Map();
  for (const row of rows) {
    const held = latest.get(row.id);
    if (!held || (held.wireOrder ?? 1) < (row.wireOrder ?? 1)) latest.set(row.id,row);
  }
  return [...latest.values()].sort((a,b)=>(a.createdOrder ?? 0)-(b.createdOrder ?? 0));
}
/** @param {Storage|null} storage @param {string} daemonId @param {string} deviceId */
export function readLedger(storage,daemonId,deviceId) {
  return latestReceipts(readAttempts(storage,daemonId,deviceId));
}
/** @param {Storage|null} storage @param {string} daemonId @param {string} deviceId @param {Outbox[]} next @param {Outbox[]} previous */
export function writeLedger(storage, daemonId, deviceId, next, previous) {
  if (!storage || !daemonId) return next;
  const old = new Map(previous.map(row=>[row.id,row])); const attempts = readAttempts(storage,daemonId,deviceId);
  const existing = latestReceipts(attempts);
  const combined = new Map(existing.map(row=>[row.id,row]));
  let wireOrder = Math.max(0,...existing.map(row=>row.wireOrder ?? 1));
  let creationOrder = Math.max(0,...existing.map(row=>row.createdOrder ?? 0),...next.map(row=>row.createdOrder ?? 0));
  const changed = next.filter(row=>old.get(row.id)!==row).map(row=>({...row}));
  for (const row of changed) {
    const retirement = readRetirement(storage,daemonId,deviceId,row);
    if (retirement && row.status === 'pending') throw new Error('Delivery was already reconciled in another tab; refresh before sending');
    const saved = combined.get(row.id); const prior = old.get(row.id);
    const fresh = !prior || prior.status === 'sent' || prior.status === 'failed';
    if (row.status === 'pending' && !saved && !fresh) throw new Error('Delivery was already reconciled in another tab; refresh before sending');
    if (row.status === 'pending' && (!prior || prior.sendId !== row.sendId || prior.status === 'failed')) row.wireOrder = ++wireOrder;
    if (row.wireOrder != null && !Number.isSafeInteger(row.wireOrder)) throw new Error('Wire creation order exhausted');
    if (saved) {
      if (JSON.stringify(saved.command) !== JSON.stringify(row.command)) throw new Error('Retry must preserve the original command');
      row.unconfirmed = saved.unconfirmed ?? row.unconfirmed; row.horizon = saved.horizon ?? null; row.attemptId = saved.attemptId; row.createdOrder = saved.createdOrder ?? row.createdOrder;
    } else if (fresh) {
      row.createdOrder = ++creationOrder;
      if (!Number.isSafeInteger(row.createdOrder)) throw new Error('Receipt creation order exhausted');
    }
    if (retirement || ['sent','failed','dismissed'].includes(row.status)) combined.delete(row.id);
    else if (saved || fresh) combined.set(row.id,row);
  }
  const pending = changed.some(row=>row.status === 'pending');
  if (pending) {
    if (combined.size > 100) throw new Error('Receipt ledger has 100 unresolved commands across open tabs');
    assertPendingCapacity(storage,daemonId,deviceId,attempts,changed);
  }
  for (const row of persistenceOrder(daemonId,deviceId,attempts,changed)) persistReceipt(storage,daemonId,deviceId,row,combined.has(row.id));
  const normalized = new Map(changed.map(row=>[row.id,row]));
  return next.map(row=>{
    const value = normalized.get(row.id) ?? row;
    if (pending) return value;
    const retired = readRetirement(storage,daemonId,deviceId,value);
    if (retired === 'failed') {
      const unresolved = readLedger(storage,daemonId,deviceId).find(item=>item.id===value.id);
      if (unresolved) return {...unresolved,status:/** @type {const} */ ('unknown'),recovery:/** @type {const} */ ('error'),error:'The latest send was rejected, but an earlier send remains unconfirmed.'};
    }
    return retired ? retiredRow(value,retired) : value;
  });
}
/** @param {string} daemonId @param {string} deviceId @param {Outbox[]} attempts @param {Outbox[]} changed */
function persistenceOrder(daemonId,deviceId,attempts,changed) {
  const saved = new Set(attempts.map(row=>receiptKey(daemonId,deviceId,row)));
  const fresh = changed.filter(row=>row.status === 'pending' && !saved.has(receiptKey(daemonId,deviceId,row)));
  if (fresh.length > 1) throw new Error('New sends must be persisted one at a time');
  return [...changed.filter(row=>row !== fresh[0]),...fresh];
}
/** @param {Storage} storage @param {string} daemonId @param {string} deviceId @param {Outbox[]} attempts @param {Outbox[]} changed */
function assertPendingCapacity(storage,daemonId,deviceId,attempts,changed) {
  const wires = new Map(attempts.map(row=>[receiptKey(daemonId,deviceId,row),row]));
  for (const row of changed) if (row.status === 'pending') wires.set(receiptKey(daemonId,deviceId,row),row);
  if (wires.size > 256) throw new Error('Delivery has 256 unconfirmed send attempts. Reconcile pending work before sending again.');
  const bytes = new TextEncoder().encode(JSON.stringify([...wires.values()])).length;
  if (bytes > 2_000_000) throw new Error('Delivery storage is full. Reconcile pending work before sending again.');
  assertRetirementCapacity(storage,daemonId,deviceId,wires.size);
}
/** @param {Storage} storage @param {string} daemonId @param {string} deviceId @param {Outbox} row @param {boolean} unresolved */
function persistReceipt(storage,daemonId,deviceId,row,unresolved) {
  const key = receiptKey(daemonId,deviceId,row);
  if (row.status === 'sent' || row.status === 'failed' || row.status === 'dismissed') {
    retire(storage,daemonId,deviceId,row,row.status);
    if (row.status === 'failed') storage.removeItem(key);
    else removeLogicalReceipt(storage,daemonId,deviceId,row.id);
    return;
  }
  if (unresolved) storage.setItem(key,JSON.stringify(row));
}
/** @param {Storage|null} storage @param {string} daemonId @param {string} deviceId @param {Outbox[]} local */
export function mergeLedger(storage,daemonId,deviceId,local) {
  const rows = new Map(readLedger(storage,daemonId,deviceId).map(row=>[row.id,row]));
  for (const row of local) {
    let durable = rows.get(row.id);
    if (durable && row.unconfirmed && !durable.unconfirmed) { durable = {...durable,unconfirmed:row.unconfirmed}; rows.set(row.id,durable); }
    const retired = readRetirement(storage,daemonId,deviceId,row);
    if (retired === 'failed' && durable) rows.set(row.id,interruptedReceipts([durable])[0]);
    else if (retired) rows.set(row.id,retiredRow(row,retired));
    else if (!durable || durable.sendId === row.sendId && durable.attemptId === row.attemptId) rows.set(row.id,row);
  }
  const unresolved = [...rows.values()].filter(row=>['pending','unknown'].includes(row.status));
  const settled = [...rows.values()].filter(row=>!['pending','unknown'].includes(row.status));
  return [...unresolved,...settled.slice(0,Math.max(0,100-unresolved.length))].map(row=>withUnconfirmed(storage,daemonId,deviceId,row));
}
/** @param {Storage|null} storage */
export function clearLocalState(storage) {
  if (!storage) return;
  const keys = [];
  for (let index = 0; index < storage.length; index++) {
    const key = storage.key(index); if (key?.startsWith('aether.remote.')) keys.push(key);
  }
  for (const key of keys) storage.removeItem(key);
}
/** @param {Outbox[]} rows */
export function interruptedReceipts(rows) {
  return rows.map(row=>row.status==='pending' ? {...row,status:/** @type {const} */ ('unknown'),error:'No durable receipt yet. Another tab may still be sending; inspect state before retrying.'} : row);
}

/** @param {Storage|null} storage @param {string} daemonId @param {string} deviceId @param {Outbox} row @param {'delivered'|'discarded'} [decision] */
export function dismissReceipt(storage, daemonId, deviceId, row, decision = 'discarded') {
  string(row.id,'receipt ID');
  if (!['unknown','failed','sent'].includes(row.status)) throw new Error('A pending command cannot be dismissed');
  if (storage && !daemonId) throw new Error('Receipt has no daemon identity');
  if (storage) { retire(storage,daemonId,deviceId,row,'dismissed',decision); removeLogicalReceipt(storage,daemonId,deviceId,row.id); }
}

/** @param {Storage} storage @param {string} daemonId @param {string} deviceId @param {string} id */
function removeLogicalReceipt(storage,daemonId,deviceId,id) {
  const base = prefix(daemonId,deviceId)+encodeURIComponent(string(id,'receipt ID'));
  const keys = [];
  for (let index=0;index<storage.length;index++) {
    const key = storage.key(index); if (key === base || key?.startsWith(base+':')) keys.push(key);
  }
  for (const key of keys) storage.removeItem(key);
}
