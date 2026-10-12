import { el } from '../../ui/dom.js';
import { panel, card, form, action, details, readList, replaceRows } from './ui.js';
import { required, memoryCommand } from './protocol.js';
import { dateLabel, words, scopeLabel } from './format.js';
import { rawDetails } from './presentation.js';

/** @typedef {import('./ui.js').Panel} Panel */

/** @param {Panel} view @param {Record<string,unknown>} memory @param {()=>Promise<void>} refresh */
function memoryRow(view, memory, refresh) {
  const id = required(memory.id, 'Memory ID');
  const root = card(words(required(memory.kind, 'Memory kind')), scopeLabel(memory.scope, view.context.store.get().agents));
  root.append(el('p', 'controls-prose', required(memory.text, 'Memory text')),
    details({ Saved: dateLabel(memory.created_at), Pin: memory.pinned === true ? 'Pinned for retrieval' : memory.pinned === false ? 'Not pinned' : 'Not reported' }));
  if (Array.isArray(memory.tags) && memory.tags.length) root.append(el('p', 'muted controls-caption', memory.tags.join(' · ')));
  if (memory.forgotten_at) root.append(el('p', 'muted', `Forgotten ${dateLabel(memory.forgotten_at)} · no longer retrieved.`));
  if (memory.superseded_by) root.append(el('p', 'muted', 'Replaced by a newer memory.'));
  root.append(rawDetails(memory));
  if (memory.forgotten_at) return root;
  const actions = el('div', 'row controls-actions');
  actions.append(action(view, memory.pinned ? 'Unpin' : 'Pin', async () => {
    const result = await view.change({ cmd: 'pin_memory', id, pinned: !memory.pinned });
    if (result) await refresh();
  }, true));
  actions.append(action(view, 'Forget', async () => {
    const result = await view.change({ cmd: 'forget_memory', id }, 'Forget this memory? Its record remains, but it will no longer be retrieved.');
    if (result) await refresh();
  }, true, true));
  root.append(actions);
  return root;
}

/** @param {Panel} view @param {string} agent @param {()=>Promise<void>} refresh */
function createForm(view, agent, refresh) {
  return form(view, 'Create memory', [
    { label: 'Memory', name: 'text', type: 'textarea', required: true },
    { label: 'Kind', name: 'kind', options: ['fact', 'decision', 'lesson', 'constraint', 'preference'] },
    { label: 'Applies to', name: 'scope', options: ['agent', 'repository', 'global'] },
    { label: 'Repository root (repository scope only)', name: 'root' },
    { label: 'Tags, separated by commas', name: 'tags' },
  ], async values => {
    const result = await view.change(memoryCommand(values, agent));
    if (result) await refresh();
  });
}

/** @param {HTMLElement} container @param {import('../../types.js').Context} context @param {string} agent */
export function mountMemories(container, context, agent) {
  required(agent, 'Agent ID');
  const view = panel(container, context);
  const list = el('div', 'stack');
  /** @type {Record<string,unknown>} */
  let filters = { scope: { scope: 'agent', id: agent }, limit: 100 };
  const refresh = async () => {
    const memories = await readList(view, { cmd: 'memories', query: filters }, 'memories');
    if (!view.alive()) return;
    replaceRows(list, memories.map(memory => memoryRow(view, memory, refresh)), 'No memories match this search.');
    view.sync();
  };
  view.body.append(card('Memories', 'Browse up to 100 matches. New entries preserve operator authorship.'), form(view, 'Search memories', [
    { label: 'Search text', name: 'text' },
    { label: 'Scope', name: 'scope', options: ['This agent', 'Global', 'All scopes'] },
    { label: 'Forgotten memories', name: 'forgotten', options: ['Hide', 'Include'] },
  ], async values => {
    filters = { limit: 100, text: values.text || null, include_forgotten: values.forgotten === 'Include',
      scope: values.scope === 'This agent' ? { scope: 'agent', id: agent } : values.scope === 'Global' ? { scope: 'global' } : null };
    await refresh();
  }, false), action(view, 'Refresh memories', refresh), list, createForm(view, agent, refresh));
  view.sync();
  void view.run(refresh);
  return view.dispose;
}
