import { markUnconfirmed, readUnconfirmed } from './unconfirmed.js';
import { KEYED_COMMANDS } from './contract.js';
import { readLedger, writeLedger } from './ledger.js';
import { submissionResolution } from './submissions.js';
import { errorText } from './validate.js';
const uncertainty = {no_horizon:'This request was sent before delivery tracking was available.',foreign_horizon:'This request belongs to another daemon history.',horizon_ahead:'The daemon history is older than this request.',forgotten:'The delivery receipt is no longer retained.'};
/** @typedef {import('./store.js').RemoteStore} Store */
/** @typedef {import('../types.js').Outbox} Outbox */
/** @param {Store} store @param {Outbox} row */
function stillPending(store, row) {
  const current = store.state.outbox.find(item=>item.id===row.id);
  if (!current || !['pending','unknown'].includes(current.status)) return false;
  if (!store.storage) return true;
  return readLedger(store.storage,store.state.daemonId,store.deviceId).some(item=>item.id===row.id && ['pending','unknown'].includes(item.status));
}
/** @param {Store} store @param {Outbox} row @param {Partial<Outbox>} patch */
function update(store, row, patch) {
  if (!stillPending(store,row)) return;
  store.patch({outbox:store.state.outbox.map(item=>item.id===row.id && item.sendId===row.sendId ? {...item,...patch} : item)});
}
/** @param {Store} store @param {number} epoch @param {Set<string>|null} [ids] */
export async function recoverSubmissions(store, epoch, ids = null) {
  const rows = store.state.outbox.filter(row=>(!ids || ids.has(row.id)) && ['pending','unknown'].includes(row.status)).sort((a,b)=>(a.createdOrder ?? 0)-(b.createdOrder ?? 0));
  for (const row of rows) {
    if (epoch !== store.epoch || store.state.connection !== 'online') return;
    if (!stillPending(store,row)) continue;
    if (!store.submissionLookup || !KEYED_COMMANDS.has(row.command.cmd) || !row.command.idempotency_key) {
      unconfirm(store,row,'unsupported','Delivery unconfirmed. This daemon or command does not support submission lookup.','unsupported'); continue;
    }
    await recoverOne(store,row,epoch);
  }
}
/** @param {Store} store @param {Outbox} row @param {number} epoch */
async function recoverOne(store, row, epoch) {
  const held = confirmation(store,row);
  if (!held) update(store,row,{status:'unknown',recovery:'resolving',error:undefined});
  let resolving = true;
  try {
    const command = {cmd:'resolve_submission',command:row.command,...(row.horizon != null ? {horizon:row.horizon} : {})};
    const resolution = submissionResolution(await store.transport.request(command));
    if (epoch !== store.epoch || !stillPending(store,row)) return;
    if (resolution.outcome === 'executed') {
      store.receive({frame:'mutation',phase:'sent',id:row.id,command:row.command,reply:resolution.reply}); return;
    }
    if (resolution.outcome === 'unknown') {
      const detail = uncertainty[/** @type {keyof typeof uncertainty} */ (resolution.reason)];
      unconfirm(store,row,resolution.reason,`Delivery unconfirmed. ${detail} Inspect the daemon before deciding whether to send again.`,'unknown'); return;
    }
    const unconfirmed = confirmation(store,row);
    if (unconfirmed) { unconfirm(store,row,unconfirmed.reason,unconfirmed.message,'unknown'); return; }
    if (store.state.connection !== 'online' || !store.state.outbox.some(item=>item.id===row.id && item.sendId===row.sendId)) return;
    resolving = false; await store.transport.request(row.command,{recovery:true});
  } catch (error) {
    if (error instanceof Error && 'uncertaintyPersistence' in error) throw error;
    if (epoch !== store.epoch) return;
    const message = `Delivery unconfirmed: ${errorText(error)}`;
    const interrupted = error instanceof Error && 'outcome' in error && !('code' in error);
    if (resolving && !interrupted) unconfirm(store,row,error instanceof Error && 'code' in error ? String(error.code) : 'invalid_reply',message,'error');
    else update(store,row,{status:'unknown',recovery:'error',error:message});
  }
}

/** @param {Store} store @param {Outbox} row */
function confirmation(store,row) {
  const current = store.state.outbox.find(item=>item.id===row.id);
  return readUnconfirmed(store.storage,store.state.daemonId,store.deviceId,row.id) ?? current?.unconfirmed ?? row.unconfirmed;
}
/** @param {Store} store @param {Outbox} row @param {string} reason @param {string} message @param {'unknown'|'error'|'unsupported'} recovery */
function unconfirm(store,row,reason,message,recovery) {
  const previous = store.state.outbox; const current = previous.find(item=>item.id===row.id);
  if (!current || !['pending','unknown'].includes(current.status)) return;
  const unconfirmed = current.unconfirmed ?? row.unconfirmed ?? {reason,message};
  const outbox = previous.map(item=>item !== current ? item : {...item,unconfirmed,...(item.sendId===row.sendId ? {status:/** @type {const} */ ('unknown'),recovery,recoveryReason:reason,error:message} : {})});
  store.state = {...store.state,outbox};
  /** @type {unknown[]} */ const errors = [];
  try { markUnconfirmed(store.storage,store.state.daemonId,store.deviceId,row.id,unconfirmed); }
  catch (error) { errors.push(error); }
  try { store.state = {...store.state,outbox:writeLedger(store.storage,store.state.daemonId,store.deviceId,outbox,previous)}; }
  catch (error) { errors.push(error); }
  for (const listener of store.listeners) listener();
  if (errors.length) throw Object.assign(new Error(`Could not save delivery uncertainty: ${errors.map(errorText).join('; ')}`,{cause:new AggregateError(errors)}),{uncertaintyPersistence:true});
}
