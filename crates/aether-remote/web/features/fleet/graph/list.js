import { button, el, invariant, number } from '../../../ui/dom.js';
import { elapsed, fleetRows, reported } from '../model.js';
import { agentLink } from '../shared.js';
import { agentState, PAGE_SIZE } from './layout.js';

/** @param {import('../../../types.js').State} state @param {import('../../../types.js').Context} context @param {number} page @param {(id:string)=>void} open */
export function compactList(state, context, page, open) {
  invariant(Number.isSafeInteger(page) && page >= 0, 'Fleet list page must be nonnegative');
  const rows = fleetRows(state.agents).slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);
  invariant(rows.length <= PAGE_SIZE, 'Fleet list rendering must be bounded');
  const names = new Map(state.agents.map(agent => [agent.id, agent.name || agent.id]));
  const attention = new Set(state.attention.map(item => item.agentId));
  const list = el('ol', 'fleet-compact-list'); list.setAttribute('aria-label', 'All agents');
  for (const { agent, parentMissing, cycle } of rows) {
    const item = el('li', 'fleet-compact-row'); item.dataset.state = agentState(agent);
    const ring = el('span', 'fleet-compact-ring', agentState(agent) === 'failed' ? '×' : agentState(agent) === 'done' ? '✓' : '');
    ring.setAttribute('aria-hidden', 'true');
    const content = el('div', 'fleet-compact-copy');
    const link = agentLink(context, agent.id, agent.name || agent.id); link.classList.add('fleet-agent');
    const metadata = el('p', 'fleet-compact-meta', `${agent.paused ? 'Paused' : agent.status} · ${agent.model.split('/').pop() || 'Model unknown'}`);
    const usage = [elapsed(agent.createdAt), reported(agent.tokens) ? `${number(agent.tokens)} tokens` : 'Tokens not reported'];
    if (agent.parent) usage.push(`From ${names.get(agent.parent) || agent.parent}`);
    link.append(metadata);
    content.append(link, el('p', 'sr-only', usage.join(' · ')));
    if (parentMissing) content.append(el('p', 'fleet-compact-usage muted', 'Parent is not included in this fleet.'));
    if (cycle) content.append(el('p', 'fleet-compact-usage muted', 'Hierarchy contains a cycle; showing each agent once.'));
    const details = button(attention.has(agent.id) ? '!' : '↗', () => open(agent.id), 'button fleet-compact-details');
    details.setAttribute('aria-label', `Details for ${agent.name || agent.id}${attention.has(agent.id) ? ', needs attention' : ''}`);
    details.dataset.attention = String(attention.has(agent.id));
    item.append(ring, content, details); list.append(item);
  }
  return list;
}
