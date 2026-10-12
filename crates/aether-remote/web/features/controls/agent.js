import { el, button, invariant } from '../../ui/dom.js';
import { panel, card, form, action, details, readList } from './ui.js';
import { modelReference, required, record } from './protocol.js';
import { words } from './format.js';
import { rawDetails } from './presentation.js';

/** @typedef {import('./ui.js').Panel} Panel */
/** @typedef {import('../../types.js').Agent} Agent */

/** @param {Panel} view @param {Agent} agent */
function lifecycle(view, agent) {
  const root = card('Agent lifecycle', 'Pause lets the current turn finish. Interrupt stops only the current turn. Cancel ends the agent and its live descendants.');
  const buttons = el('div', 'row controls-actions');
  const subtree = el('input');
  subtree.type = 'checkbox';
  const subtreeLabel = el('label', 'row');
  subtreeLabel.append(subtree, document.createTextNode('Include descendants when pausing or resuming'));
  const actions = ['interrupt', 'pause', 'resume', 'restart', 'wake', 'compact'];
  for (const cmd of actions) {
    const label = cmd[0].toUpperCase() + cmd.slice(1);
    buttons.append(action(view, label, async () => {
      const command = { cmd, agent: agent.id, ...(['pause', 'resume'].includes(cmd) ? { subtree: subtree.checked } : {}) };
      const confirmation = cmd === 'restart' ? `Restart ${agent.name} as a new agent from its last checkpoint?`
        : cmd === 'interrupt' ? `Stop the running turn for ${agent.name}?` : undefined;
      await view.change(command, confirmation);
    }, true, cmd === 'interrupt'));
  }
  buttons.append(action(view, 'Archive', async () => {
    await view.change({ cmd: 'archive', target: { agent: agent.id }, archived: true }, `Archive ${agent.name}? Only finished agents can be archived. Its history remains available.`);
  }, true, true));
  buttons.append(action(view, 'Restore from archive', async () => {
    await view.change({ cmd: 'archive', target: { agent: agent.id }, archived: false });
  }, true));
  root.append(subtreeLabel, buttons, form(view, 'Cancel agent', [
    { label: 'Reason (optional)', name: 'reason', type: 'textarea' },
  ], async values => {
    await view.change({ cmd: 'cancel', agent: agent.id, reason: values.reason || null }, `Cancel ${agent.name} and its live descendants? Running work will stop.`);
  }));
  return root;
}

/** @param {Panel} view @param {Agent} agent */
function configuration(view, agent) {
  const root = card('Identity and instructions');
  const config = agent.raw.config == null ? {} : record(agent.raw.config);
  root.append(form(view, 'Rename agent', [{ label: 'Name', name: 'name', value: agent.name, required: true }], async values => {
    await view.change({ cmd: 'update_agent', id: agent.id, patch: { name: required(values.name, 'Name') } });
  }));
  root.append(form(view, 'Save instructions', [{ label: 'Agent instructions', name: 'instructions', type: 'textarea', value: typeof config.instructions === 'string' ? config.instructions : '' }], async values => {
    await view.change({ cmd: 'update_agent', id: agent.id, patch: { instructions: values.instructions } });
  }));
  return root;
}

/** @param {Panel} view @param {Agent} agent */
function modelSettings(view, agent) {
  const root = card('Model and reasoning', 'Model changes may require a checkpoint. The daemon validates provider support.');
  const catalogue = el('div', 'stack');
  root.append(action(view, 'Browse available models', async () => {
    const models = await readList(view, { cmd: 'list_models' }, 'models');
    if (!view.alive()) return;
    catalogue.replaceChildren(...models.map(model => {
      const reference = record(model.model);
      const value = `${required(reference.provider, 'Provider')}/${required(reference.model, 'Model')}`;
      const choice = button(`${value}${model.configured ? '' : ' · not configured'}`,() => {
        const input = root.querySelector('input[name="model"]');
        invariant(input instanceof HTMLInputElement, 'The model field must be present.');
        input.value = value; input.focus(); input.scrollIntoView({block:'center'});
      });
      choice.disabled = model.configured !== true;
      return choice;
    }));
    if (!models.length) catalogue.append(el('p', 'muted', 'No models reported.'));
  }), catalogue);
  root.append(form(view, 'Change model', [
    { label: 'Provider/model', name: 'model', value: agent.model, required: true },
  ], async values => {
    await view.change({ cmd: 'update_agent', id: agent.id, patch: { model: modelReference(values.model) } });
  }));
  root.append(form(view, 'Change effort', [
    { label: 'Reasoning effort', name: 'effort', value: agent.effort, options: ['low', 'medium', 'high', 'x_high', 'max'] },
  ], async values => {
    invariant(['low', 'medium', 'high', 'x_high', 'max'].includes(values.effort), 'Choose a supported reasoning effort.');
    await view.change({ cmd: 'update_agent', id: agent.id, patch: { effort: values.effort } });
  }));
  return root;
}

/** @param {Panel} view @param {Agent} agent */
function approvals(view, agent) {
  const root = card('Approval policy', 'Select which tool side effects require approval. Host and parent restrictions still apply.');
  const config = agent.raw.config == null ? {} : record(agent.raw.config);
  const policy = config.approval == null ? {} : record(config.approval);
  const current = Array.isArray(policy.require_for) ? policy.require_for : [];
  const classes = ['read_only', 'idempotent_mutation', 'mutation', 'external'];
  root.append(form(view, 'Apply approval policy', [
    ...classes.map(name => ({ name, label: name.replaceAll('_', ' '), options: ['Do not require', 'Require approval'], value: current.includes(name) ? 'Require approval' : 'Do not require' })),
    { label: 'Reach', name: 'reach', options: ['This agent', 'Agent and descendants'] },
  ], async values => {
    const require_for = classes.filter(name => values[name] === 'Require approval');
    await view.change({ cmd: 'set_approvals', agent: agent.id, policy: { require_for }, subtree: values.reach === 'Agent and descendants' },
      `Apply this approval policy to ${values.reach.toLowerCase()}? Required classes: ${require_for.join(', ') || 'none (unrestricted approvals)'}.`);
  }));
  return root;
}

/** @param {HTMLElement} container @param {import('../../types.js').Context} context @param {string} agentId */
export function mountAgent(container, context, agentId) {
  const agent = context.store.get().agents.find(item => item.id === agentId);
  invariant(agent, 'The selected agent is no longer available.');
  required(agent.id, 'Agent ID');
  const view = panel(container, context);
  const summary = el('div');
  const update = () => {
    const current = context.store.get().agents.find(item => item.id === agentId);
    const expanded = summary.querySelector('details')?.open === true;
    summary.replaceChildren(current ? agentSummary(current)
      : el('p', 'muted', 'Agent is no longer in the live view. Its stored history remains available.'));
    const disclosure = summary.querySelector('details');
    if (disclosure) disclosure.open = expanded;
  };
  try { view.body.append(summary, lifecycle(view, agent), modelSettings(view, agent), approvals(view, agent), configuration(view, agent)); }
  catch (error) { view.dispose(); throw error; }
  const unsubscribe = context.store.subscribe(update);
  update();
  view.sync();
  return () => { unsubscribe(); view.dispose(); };
}

/** @param {Agent} agent */
function agentSummary(agent) {
  required(agent.id, 'Agent ID');
  const config = agent.raw.config == null ? {} : record(agent.raw.config);
  const policy = config.approval == null ? {} : record(config.approval);
  const classes = Array.isArray(policy.require_for) ? policy.require_for : null;
  const root = card('Current state', agent.paused ? 'Pause requested · the current turn may still be finishing.' : '');
  const approval = classes ? (classes.length ? classes.map(words).join(', ') : 'No required approvals') : 'Not reported';
  root.append(details({ Status: words(agent.status), Model: agent.model, Reasoning: agent.effort === 'x_high' ? 'Extra high' : words(agent.effort), 'Approval required': approval }), rawDetails(agent.raw));
  return root;
}
