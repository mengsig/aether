import { assertSubmissionLock, SubmissionBusy } from './submission-locks.js';
import { markUnconfirmed, readUnconfirmed } from './unconfirmed.js';
import { KEYED_COMMANDS } from './contract.js';
import { readLedger, writeLedger } from './ledger.js';
import { submissionResolution } from './submissions.js';
import { errorText } from './validate.js';
const uncertainty = {no_horizon:'This request was sent before delivery tracking was available.',foreign_horizon:'This request belongs to another daemon history.',horizon_ahead:'The daemon history is older than this request.',forgotten:'The delivery receipt is no longer retained.'};
/** @typedef {import('./store.js').RemoteStore} Store */
/** @typedef {import('../types.js').Outbox} Outbox */
/** @typedef {import('./submission-locks.js').SubmissionLock} Lease */
/** @param {Store} store @param {Outbox} row */
function stillPending(store, row) {
  const current = store.state.outbox.find(item=>item.id===row.id);
  if (!current || !['pending','unknown'].includes(current.status)) return false;
  if (!store.storage) return true;
  return readLedger(store.storage,store.state.daemonId,store.deviceId).some(item=>item.id===row.id && ['pending','unknown'].includes(item.status));
}
/** @param {Store} store @param {Outbox} row @param {Partial<Outbox>} patch @param {Lease} lease */
function update(store, row, patch,lease) {
  if (!stillPending(store,row)) return;
  store.patch({outbox:store.state.outbox.map(item=>item.id===row.id && item.sendId===row.sendId ? {...item,...patch} : item)},lease);
}
/** @param {Store} store @param {number} epoch @param {Set<string>|null} [ids] */
export async function recoverSubmissions(store, epoch, ids = null) {
  const rows = store.state.outbox.filter(row=>(!ids || ids.has(row.id)) && ['pending','unknown'].includes(row.status)).sort((a,b)=>(a.createdOrder ?? 0)-(b.createdOrder ?? 0));
  const role = store.state.role; const signal = store.submissionAbort.signal;
  for (const row of rows) {
    if (!currentSession(store,epoch,role)) return;
    try {
      await store.withReceiptLock(row.id,async lease=>{
        store.cancelDeferredSubmission(row.id);
        if (!currentSession(store,epoch,role) || !stillPending(store,row)) return;
        const durable = store.storage ? readLedger(store.storage,store.state.daemonId,store.deviceId).find(item=>item.id===row.id) : row;
        if (!durable) return;
        const current = {...durable,status:/** @type {const} */ ('unknown'),unconfirmed:confirmation(store,durable)};
        store.state = {...store.state,outbox:store.state.outbox.map(item=>item.id===row.id ? current : item)};
        if (!store.submissionLookup || !KEYED_COMMANDS.has(current.command.cmd) || !current.command.idempotency_key) {
          unconfirm(store,current,'unsupported','Delivery unconfirmed. This daemon or command does not support submission lookup.','unsupported',lease); return;
        }
        await recoverOne(store,current,epoch,role,lease);
      },{signal,ifAvailable:true});
    } catch (error) {
      if (signal.aborted && error === signal.reason) return;
      if (error instanceof SubmissionBusy) { store.deferSubmission(row.id); continue; }
      throw error;
    }
  }
}
/** @param {Store} store @param {number} epoch @param {string} role */
function currentSession(store,epoch,role) {
  return epoch === store.epoch && store.state.connection === 'online' && store.state.role === role;
}
/** @param {Store} store @param {Outbox} row @param {number} epoch @param {string} role @param {Lease} lease */
async function recoverOne(store, row, epoch,role,lease) {
  if (!lease.exclusive) unconfirm(store,row,'locks_unavailable','Delivery unconfirmed. This browser cannot coordinate delivery across tabs. Inspect the daemon before choosing Send again.','unsupported',lease);
  const held = confirmation(store,row);
  if (!held) update(store,row,{status:'unknown',recovery:'resolving',error:undefined},lease);
  let resolving = true;
  try {
    const command = {cmd:'resolve_submission',command:row.command,...(row.horizon != null ? {horizon:row.horizon} : {})};
    const resolution = submissionResolution(await store.transport.request(command));
    if (!currentSession(store,epoch,role) || !stillPending(store,row)) return;
    if (resolution.outcome === 'executed') {
      await store.receive({frame:'mutation',phase:'sent',id:row.id,command:row.command,reply:resolution.reply,submissionLock:lease}); return;
    }
    if (resolution.outcome === 'unknown') {
      const detail = uncertainty[/** @type {keyof typeof uncertainty} */ (resolution.reason)];
      unconfirm(store,row,resolution.reason,`Delivery unconfirmed. ${detail} Inspect the daemon before deciding whether to send again.`,'unknown',lease); return;
    }
    const unconfirmed = confirmation(store,row);
    if (unconfirmed) { unconfirm(store,row,unconfirmed.reason,unconfirmed.message,'unknown',lease); return; }
    if (store.state.connection !== 'online' || !store.state.outbox.some(item=>item.id===row.id && item.sendId===row.sendId)) return;
    resolving = false; await store.transport.request(row.command,{recovery:true,submissionLock:lease});
  } catch (error) {
    if (error instanceof Error && 'uncertaintyPersistence' in error) throw error;
    if (!currentSession(store,epoch,role)) return;
    const message = `Delivery unconfirmed: ${errorText(error)}`;
    const interrupted = error instanceof Error && 'outcome' in error && !('code' in error);
    if (resolving && !interrupted) unconfirm(store,row,error instanceof Error && 'code' in error ? String(error.code) : 'invalid_reply',message,'error',lease);
    else update(store,row,{status:'unknown',recovery:'error',error:message},lease);
  }
}

/** @param {Store} store @param {Outbox} row */
function confirmation(store,row) {
  const current = store.state.outbox.find(item=>item.id===row.id);
  return readUnconfirmed(store.storage,store.state.daemonId,store.deviceId,row.id) ?? current?.unconfirmed ?? row.unconfirmed;
}
/** @param {Store} store @param {Outbox} row @param {string} reason @param {string} message @param {'unknown'|'error'|'unsupported'} recovery @param {Lease} lease */
function unconfirm(store,row,reason,message,recovery,lease) {
  assertSubmissionLock(lease,{daemonId:store.state.daemonId,deviceId:store.deviceId},row.id);
  if (!stillPending(store,row)) return;
  const previous = store.state.outbox; const current = previous.find(item=>item.id===row.id);
  if (!current || !['pending','unknown'].includes(current.status)) return;
  const unconfirmed = current.unconfirmed ?? row.unconfirmed ?? {reason,message};
  const outbox = previous.map(item=>item !== current ? item : {...item,unconfirmed,...(item.sendId===row.sendId ? {status:/** @type {const} */ ('unknown'),recovery,recoveryReason:reason,error:message} : {})});
  store.state = {...store.state,outbox};
  /** @type {unknown[]} */ const errors = [];
  try { markUnconfirmed(store.storage,store.state.daemonId,store.deviceId,row.id,unconfirmed); }
  catch (error) { errors.push(error); }
  try {
    const saved = writeLedger(store.storage,store.state.daemonId,store.deviceId,outbox.filter(item=>item.id===row.id),previous.filter(item=>item.id===row.id));
    store.state = {...store.state,outbox:outbox.map(item=>item.id===row.id ? saved[0] : item)};
  }
  catch (error) { errors.push(error); }
  for (const listener of store.listeners) listener();
  if (errors.length) throw Object.assign(new Error(`Could not save delivery uncertainty: ${errors.map(errorText).join('; ')}`,{cause:new AggregateError(errors)}),{uncertaintyPersistence:true});
}
