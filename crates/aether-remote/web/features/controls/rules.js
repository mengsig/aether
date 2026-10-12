import { el } from '../../ui/dom.js';
import { panel, card, form, action, details, readList, replaceRows } from './ui.js';
import { required, record, ruleCommand } from './protocol.js';
import { dateLabel, scopeLabel, words } from './format.js';
import { rawDetails } from './presentation.js';

/** @typedef {import('./ui.js').Panel} Panel */

/** @param {Panel} view @param {Record<string,unknown>} rule @param {()=>Promise<void>} refresh */
function ruleRow(view, rule, refresh) {
  const id = required(rule.id, 'Approval rule ID');
  const decision = required(rule.decision, 'Rule decision');
  const root = card(decision === 'approved' ? 'Allow matching calls' : decision === 'denied' ? 'Deny matching calls' : words(decision),
    scopeLabel(rule.scope, view.context.store.get().agents));
  const scope = record(rule.scope);
  const matcher = rule.match == null ? null : record(rule.match);
  root.append(details({ Tool: matcher ? matcher.tool ?? 'Every tool' : 'Not reported',
    Argument: matcher ? matcher.argument ?? 'Any argument' : 'Not reported',
    Resource: matcher ? matcher.resource_prefix ?? 'Any resource' : 'Not reported',
    Status: rule.revoked ? 'Revoked' : rule.revoked === null ? 'Active' : 'Not reported', Created: dateLabel(rule.created_at) }));
  if (typeof rule.note === 'string' && rule.note) root.append(el('p', 'controls-prose', rule.note));
  root.append(rawDetails(rule));
  if (scope.scope === 'everywhere') root.append(el('p', 'muted', 'Everywhere rules are managed on the Aether host.'));
  if (!rule.revoked && scope.scope !== 'everywhere') root.append(action(view, 'Revoke rule', async () => {
    const result = await view.change({ cmd: 'revoke_approval_rule', id }, `Revoke approval rule ${id}? Future matching calls will no longer use this decision.`);
    if (result) await refresh();
  }, true, true));
  return root;
}

/** @param {Panel} view @param {string} agent @param {()=>Promise<void>} refresh */
function createForm(view, agent, refresh) {
  const run = view.context.store.get().agents.find(item => item.id === agent)?.run || '';
  return form(view, 'Add approval rule', [
    { label: 'Decision', name: 'decision', options: ['denied', 'approved'] },
    { label: 'Scope', name: 'scope', options: ['agent', 'subtree', 'run'] },
    { label: 'Run ID (run scope only)', name: 'run', value: run },
    { label: 'Tool matcher (blank matches every tool)', name: 'tool' },
    { label: 'Argument matcher (optional)', name: 'argument' },
    { label: 'Resource prefix (optional)', name: 'resource' },
    { label: 'Note (optional)', name: 'note', type: 'textarea' },
  ], async values => {
    const command = ruleCommand(values, agent);
    const confirmation = `Add a ${values.decision} rule for ${values.scope}? Tool: ${values.tool || 'every tool'}; argument: ${values.argument || 'any'}; resource: ${values.resource || 'any'}.`;
    const result = await view.change(command, confirmation);
    if (result) await refresh();
  });
}

/** @param {HTMLElement} container @param {import('../../types.js').Context} context @param {string} agent */
export function mountRules(container, context, agent) {
  required(agent, 'Agent ID');
  const view = panel(container, context);
  const list = el('div', 'stack');
  const refresh = async () => {
    const rules = await readList(view, { cmd: 'list_approval_rules', include_revoked: true }, 'approval_rules');
    if (!view.alive()) return;
    replaceRows(list, rules.map(rule => ruleRow(view, rule, refresh)), 'No remembered approval rules.');
    view.sync();
  };
  view.body.append(card('Approval rules', 'Remembered decisions. An empty matcher applies to every tool call within its scope. Creating rules for everywhere requires host access.'),
    action(view, 'Refresh rules', refresh), list, createForm(view, agent, refresh));
  view.sync();
  void view.run(refresh);
  return view.dispose;
}
