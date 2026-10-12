import {button,el,errorText,invariant} from './dom.js';

/** @param {import('../types.js').Outbox} item */
export function checkingDelivery(item) {
  if (item.unconfirmed) return false;
  return item.recovery==='resolving' || (item.status==='unknown' && item.command.idempotency_key!=null && item.horizon!=null && item.recovery==null);
}

/** @param {import('../types.js').Outbox} item @param {import('../types.js').Context} context */
export function deliveryActions(item,context) {
  invariant(['pending','failed','unknown'].includes(item.status), 'Only unresolved deliveries have review actions');
  invariant(typeof item.command.cmd==='string' && item.id.length>0, 'Delivery review requires the original request and receipt');
  const root=el('div','stack');const actions=el('div','row');const status=el('p','small muted');
  status.setAttribute('role','status');root.append(actions,status);
  if (item.status==='pending' || checkingDelivery(item)) return root;
  const run=(/** @type {()=>Promise<void>} */ action)=>{void action().catch(error=>{status.textContent=errorText(error);});};
  if (item.command.idempotency_key && context.store.get().role==='operator') {
    const retry=button('Send again…',()=>run(()=>retryDelivery(item,context)));
    retry.disabled=context.store.get().connection!=='online';actions.append(retry);
  }
  if (item.status==='unknown') actions.append(button('Mark delivered…',()=>run(()=>dismissDelivery(item,context,true))));
  actions.append(button('Discard…',()=>run(()=>dismissDelivery(item,context,false))));
  return root;
}

/** @param {import('../types.js').Outbox} item @param {import('../types.js').Context} context */
function requireUnconfirmed(item,context) {
  const current=context.store.get().outbox.find(row=>row.id===item.id);
  if (!current || !['failed','unknown'].includes(current.status) || checkingDelivery(current)) throw new Error('Delivery status changed. Review the latest receipt.');
  if (JSON.stringify(current.command)!==JSON.stringify(item.command)) throw new Error('The original request changed. Review its latest details.');
  return current;
}

/** @param {import('../types.js').Outbox} item @param {import('../types.js').Context} context */
async function retryDelivery(item,context) {
  requireUnconfirmed(item,context);
  await context.store.refresh();
  if (!await context.confirm('Send this request again?', 'Check current state first. The original key is reused, but sending again can repeat work if its receipt was forgotten.')) return;
  const current=requireUnconfirmed(item,context);
  if (context.store.get().role!=='operator' || context.store.get().connection!=='online') throw new Error('Reconnect with operator access before sending.');
  if (!current.command.idempotency_key) throw new Error('The original delivery key is unavailable.');
  await context.command(current.command);
}

/** @param {import('../types.js').Outbox} item @param {import('../types.js').Context} context @param {boolean} delivered */
async function dismissDelivery(item,context,delivered) {
  requireUnconfirmed(item,context);
  const title=delivered?'Mark this delivery as confirmed?':'Discard this local delivery record?';
  const detail=delivered?'Only continue if you have checked that the request took effect. This clears the local record; it sends nothing to the daemon.':'This removes the local record. It does not undo work or cancel a request that may already have arrived.';
  if (!await context.confirm(title,detail)) return;
  requireUnconfirmed(item,context);await context.store.dismissReceipt(item.id,delivered?'delivered':'discarded');
}
