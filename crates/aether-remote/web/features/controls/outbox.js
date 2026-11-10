import {el,button} from '../../ui/dom.js';
import {deliveryActions,checkingDelivery} from '../../ui/delivery-actions.js';

/** @param {import('../../types.js').Context} context */
export function deliveryReview(context) {
  const root = el('section','card stack');
  root.setAttribute('aria-label','Delivery status');
  const list = el('div','stack');
  root.append(el('h2','','Delivery status'),el('p','muted small','Aether checks requests after reconnecting. If the daemon cannot confirm a delivery, review it here.'),list);
  let signature = '';
  const render = () => {
    const items = context.store.get().outbox.filter(item => ['pending','unknown','failed'].includes(item.status));
    const current = `${context.store.get().role}:${context.store.get().connection}:${JSON.stringify(items)}`;
    if (current === signature) return; signature = current;
    list.replaceChildren();
    for (const item of items) list.append(receiptCard(item,context));
    if (!items.length) list.append(el('p','muted','No unresolved deliveries.'));
  };
  const unsubscribe = context.store.subscribe(render); render();
  return {root,destroy:unsubscribe};
}

/** @param {import('../../types.js').Outbox} item @param {import('../../types.js').Context} context */
function receiptCard(item,context) {
  const row = el('article','stack'); const record = el('details');
  record.append(el('summary','',item.command.cmd.replaceAll('_',' ')),el('pre','',JSON.stringify(item.command,null,2)));
  const resolving=checkingDelivery(item);
  const label=resolving ? 'Checking delivery…' : item.status==='unknown' ? 'Delivery unconfirmed' : item.status==='pending' ? 'Sending…' : 'Not delivered';
  row.append(el('span','badge',label),record);
  if (item.error && !resolving) row.append(el('p','muted small',item.error));
  if (item.retentionRisk) row.append(el('p','muted small',item.retentionRisk));
  const actions = el('div','row');
  if (item.agentId.startsWith('agt_')) actions.append(button('Review agent',() => context.navigate(`agent/${item.agentId}`)));
  row.append(actions,deliveryActions(item,context)); return row;
}
