import {button, el, errorText, invariant} from '../../ui/dom.js';
import {actorLabel} from '../../core/actors.js';
import {array, integer, object, string} from '../../core/validate.js';

/** @param {import('../../types.js').Context} context @param {string} agentId */
export function agentInbox(context, agentId) {
  invariant(agentId.length > 0,'Inbox requires an agent');
  const root = el('details','conversation-inbox');
  const list = el('div','stack'); const status = el('p','muted small'); status.setAttribute('role','status');
  const reload = button('Refresh deliveries',() => { void load(); });
  root.append(el('summary','muted','Inbox deliveries'),el('p','muted small','Messages, decisions and task results delivered to this agent.'),reload,status,list);
  let generation = 0;
  async function load() {
    if (!root.open || context.store.get().connection !== 'online') { status.textContent = 'Reconnect to read deliveries.'; return; }
    const current = ++generation; reload.disabled = true; status.textContent = 'Reading deliveries…';
    try {
      const reply = await context.transport.request({cmd:'get_inbox',agent:agentId,after:0});
      invariant(reply.reply === 'inbox','Invalid inbox reply');
      const items = array(reply.data).map(value => inboxRecord(value,context.store.get().agents));
      if (!root.isConnected || current !== generation) return;
      list.replaceChildren(...items.slice(-100).map(item => deliveryCard(item)));
      status.textContent = items.length ? items.length > 100 ? `Showing the latest 100 of ${items.length} deliveries.` : `${items.length} ${items.length === 1 ? 'delivery' : 'deliveries'}` : 'No deliveries reported.';
    } catch (error) { if (root.isConnected && current === generation) status.textContent = errorText(error); }
    finally { if (current === generation) reload.disabled = false; }
  }
  root.addEventListener('toggle',() => { if (root.open) void load(); else { ++generation; reload.disabled = false; } });
  return root;
}

/** @param {unknown} value @param {import('../../types.js').Agent[]} [agents] */
export function inboxRecord(value, agents = []) {
  const raw = object(value); const kind = object(raw.kind); const type = string(kind.kind);
  integer(raw.seq); string(raw.delivered_at); string(raw.agent_id);
  if (!Number.isFinite(Date.parse(string(raw.delivered_at)))) throw new Error('Invalid inbox delivery time');
  let title = type.replaceAll('_',' '); let by = ''; let detail = '';
  if (type === 'approval_resolved') {
    if (kind.decision !== 'approved' && kind.decision !== 'denied') throw new Error('Invalid approval decision');
    title = kind.decision === 'approved' ? 'Approval granted' : 'Approval denied';
    by = `Decided by: ${actorLabel(kind.by,agents)}`;
    detail = typeof kind.note === 'string' ? kind.note : '';
  } else if (type === 'message') {
    by = `From: ${actorLabel(kind.from,agents)}`;
    detail = array(kind.content).map(inboxText).join('\n');
  }
  return {title,by,detail,at:string(raw.delivered_at),raw};
}

/** @param {unknown} value */
function inboxText(value) {
  const block = object(value); const type = string(block.type);
  if (type !== 'text') return `[${type}]`;
  if (typeof block.text !== 'string') throw new Error('Invalid inbox message text');
  return block.text;
}

/** @param {ReturnType<typeof inboxRecord>} item */
function deliveryCard(item) {
  const card = el('article','card stack');
  card.append(el('strong','',item.title),el('time','muted small',new Date(item.at).toLocaleString()));
  if (item.by) card.append(el('p','muted small',item.by));
  if (item.detail) card.append(el('p','',item.detail));
  const exact = el('details'); exact.append(el('summary','','Original delivery'),el('pre','',JSON.stringify(item.raw,null,2)));
  card.append(exact); return card;
}
