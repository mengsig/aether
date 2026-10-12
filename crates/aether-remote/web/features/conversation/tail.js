import {el, invariant} from '../../ui/dom.js';
import {deliveryActions,checkingDelivery} from '../../ui/delivery-actions.js';
import {deliveryLabel} from './state.js';

/** @param {HTMLElement} root @param {import('../../types.js').Context} context @param {string} agentId */
export function transcriptTail(root, context, agentId) {
  invariant(root instanceof HTMLElement, 'Live responses require a transcript tail');
  invariant(agentId.length > 0, 'Live responses require an agent');
  const live = el('div', 'conversation-streams');
  const deliveries = el('div', 'conversation-deliveries');
  const recent=el('details','conversation-recent');const recentList=el('div');
  recent.append(el('summary','muted small','Recent deliveries'),recentList);
  root.append(deliveries,recent,live);
  /** @type {Map<string,{node:HTMLElement,text:HTMLElement,thinking:HTMLElement}>} */
  const streams = new Map();
  /** @type {Map<string,{item:import('../../types.js').Outbox,role:string,connection:string,node:HTMLElement}>} */
  const outbox = new Map();
  return {
    /** @param {import('../../types.js').State} state */
    update(state) {
      const active = Object.values(state.streams).filter(stream => stream.agentId === agentId);
      for (const [key, row] of streams) {
        if (active.some(stream => stream.invocation === key)) continue;
        row.node.remove(); streams.delete(key);
      }
      for (const stream of active) {
        let row = streams.get(stream.invocation);
        if (!row) {
          row = streamCard();
          streams.set(stream.invocation, row);
          live.append(row.node);
        }
        if (row.text.textContent !== stream.text) row.text.textContent = stream.text;
        if (row.thinking.textContent !== stream.thinking) row.thinking.textContent = stream.thinking;
        if (row.thinking.parentElement) row.thinking.parentElement.hidden = !stream.thinking;
      }
      const pending = state.outbox.filter(item => item.agentId === agentId && item.status !== 'dismissed' && ['send_message','reply_to_question'].includes(item.command.cmd));
      recent.hidden=!pending.some(item=>item.status==='sent');
      for (const [key, row] of outbox) {
        if (pending.some(item => item.id === key)) continue;
        row.node.remove(); outbox.delete(key);
      }
      for (const item of pending) {
        const old = outbox.get(item.id);
        if (old?.item === item && old.role === state.role && old.connection === state.connection) continue;
        const node = deliveryCard(item, context);
        old?.node.remove();
        (item.status==='sent' ? recentList : deliveries).append(node);
        outbox.set(item.id, {item, role:state.role, connection:state.connection, node});
      }
    },
  };
}

function streamCard() {
  const node = el('article', 'conversation-message conversation-stream');
  node.setAttribute('aria-label', 'Agent response in progress');
  node.setAttribute('aria-busy', 'true');
  const text = el('div', 'conversation-stream-text');
  const thinking = el('div', 'conversation-stream-text muted');
  const details = el('details');
  details.append(el('summary', 'muted', 'Reasoning in progress'), thinking);
  const indicator = el('p', 'conversation-stream-indicator', 'Responding');
  indicator.setAttribute('role', 'status');
  node.append(indicator, details, text);
  return {node, text, thinking};
}

/** @param {import('../../types.js').Outbox} item @param {import('../../types.js').Context} context */
function deliveryCard(item, context) {
  invariant(item.id.length > 0, 'Delivery receipts require stable keys');
  invariant(typeof item.text === 'string', 'Delivery receipts require original message text');
  const node = el('article', `conversation-delivery conversation-delivery-${item.status}`);
  const resolving=checkingDelivery(item);
  node.append(el('p','conversation-delivery-bubble',item.text),el('p','muted',resolving?'Checking delivery…':deliveryLabel(item.status)));
  if (item.error && !resolving) node.append(el('p', 'conversation-error', item.error));
  if (item.retentionRisk) node.append(el('p','conversation-error',item.retentionRisk));
  if (item.status==='failed' || item.status==='unknown') node.append(deliveryActions(item,context));
  return node;
}
