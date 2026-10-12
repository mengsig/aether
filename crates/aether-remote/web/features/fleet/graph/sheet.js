import { button, el, errorText, invariant, money, number } from '../../../ui/dom.js';
import { elapsed, reported } from '../model.js';
import { metric, statusBadge, timeLabel } from '../shared.js';
import { agentState } from './layout.js';

/** @typedef {import('../../../types.js').Context} Context */
/** @typedef {import('../../../types.js').Agent} Agent */

/** @param {Context} context @param {string} id */
export function interruptible(context, id) {
  const state = context.store.get();
  invariant(state.role === 'operator', 'Observer access cannot interrupt agents.');
  invariant(state.connection === 'online', 'Reconnect before interrupting an agent.');
  const agent = state.agents.find(item => item.id === id);
  invariant(agent, 'This agent is no longer in the fleet.');
  invariant(agentState(agent) === 'running', 'This agent is no longer running.');
  return agent;
}

/** @param {Context} context @param {string} id @param {()=>boolean} alive */
export async function interruptAgent(context, id, alive) {
  const agent = interruptible(context, id);
  if (!await context.confirm('Interrupt this turn?', `Stop the running turn for ${agent.name || agent.id}? The agent and its history will remain available.`)) return;
  invariant(alive(), 'The agent details were closed. Open them again to interrupt.');
  interruptible(context, id);
  await context.command({ cmd: 'interrupt', agent: id });
  context.toast('Interrupt requested');
}

/** @param {HTMLElement} host @param {Context} context */
export function createAgentSheet(host, context) {
  invariant(host instanceof HTMLElement, 'Agent details require a host');
  invariant(typeof context.confirm === 'function', 'Agent interrupts require confirmation');
  let selected = '';
  let pending = false;
  /** @type {Element|null} */ let previous = null;
  const dialog = el('dialog', 'fleet-agent-sheet');
  dialog.setAttribute('aria-labelledby', 'fleet-agent-sheet-title');
  const title = el('h2'); title.id = 'fleet-agent-sheet-title'; title.tabIndex = -1;
  const detail = el('div'); const actions = el('div', 'fleet-sheet-actions');
  const close = button('Close', () => dialog.close(), 'button quiet');
  const top = el('div', 'fleet-sheet-top'); top.append(el('span', 'eyebrow', 'Agent details'), close);
  dialog.append(top, title, detail, actions); host.append(dialog);
  dialog.addEventListener('close', () => {
    selected = '';
    if (previous instanceof HTMLElement || previous instanceof SVGElement) {
      if (previous.isConnected) previous.focus({ preventScroll: true });
    }
  });
  const update = () => {
    if (!selected) return;
    const state = context.store.get(); const agent = state.agents.find(item => item.id === selected);
    if (!agent) { selected = ''; dialog.close(); context.toast('This agent is no longer in the fleet.'); return; }
    const focused = document.activeElement?.getAttribute('data-sheet-action');
    title.textContent = agent.name || agent.id;
    detail.replaceChildren(sheetDetails(agent, state.activity));
    const open = button('Open chat', () => {dialog.close();context.navigate(`agent/${encodeURIComponent(agent.id)}`);}, 'button primary');
    open.dataset.sheetAction = 'chat'; actions.replaceChildren(open);
    const approval = state.attention.find(item => item.agentId === agent.id && item.kind === 'approval');
    if (approval) {
      const approve = button(state.role === 'operator' ? 'Approve…' : 'View approval', () => {dialog.close();context.navigate(`attention?item=${encodeURIComponent(approval.id)}`);});
      approve.dataset.sheetAction = 'approve'; actions.append(approve);
    }
    if (state.role === 'operator' && agentState(agent) === 'running') {
      const interrupt = button(pending ? 'Interrupting…' : 'Interrupt', () => {
        pending = true; update();
        void interruptAgent(context, agent.id, () => dialog.isConnected && dialog.open && selected === agent.id)
          .catch(error => context.toast(errorText(error))).finally(() => { pending = false; update(); });
      }, 'button danger');
      interrupt.dataset.sheetAction = 'interrupt'; interrupt.disabled = pending || state.connection !== 'online'; actions.append(interrupt);
    }
    for (const action of actions.querySelectorAll('button')) if (action.dataset.sheetAction === focused) action.focus({ preventScroll: true });
  };
  return { update, open: (/** @type {string} */ id) => {
    previous = document.activeElement; selected = id; update();
    if (!selected) return;
    dialog.showModal(); title.focus();
  }, dispose: () => { if (dialog.open) dialog.close(); dialog.remove(); } };
}

/** @param {Agent} agent @param {import('../../../types.js').Activity[]} activity */
function sheetDetails(agent, activity) {
  invariant(agent.id.length > 0, 'Agent details require an ID');
  invariant(Array.isArray(activity), 'Agent activity must be an array');
  const root = el('div', 'fleet-sheet-detail');
  const status = el('div', 'row'); status.append(statusBadge(agent.status));
  if (agent.paused) status.append(statusBadge('Paused'));
  root.append(status, el('p', 'muted', agent.role || 'Role not reported'));
  const model = el('dl', 'fleet-sheet-model');
  model.append(metric('Model', agent.model || 'Not reported'), metric('Effort', agent.effort || 'Not reported'));
  const metrics = el('dl', 'fleet-sheet-metrics');
  metrics.append(metric('Age', elapsed(agent.createdAt)),
    metric('Tokens', reported(agent.tokens) ? number(agent.tokens) : 'Not reported'),
    metric(agent.costComplete ? 'Cost' : 'Cost · partial', reported(agent.cost) ? money(agent.cost) : 'Not reported'));
  const last = activity.filter(item => item.agentId === agent.id).reduce((latest, item) => !latest || item.seq > latest.seq ? item : latest, /** @type {import('../../../types.js').Activity|null} */ (null));
  const latest = el('div', 'fleet-sheet-activity');
  latest.append(el('h3', '', 'Last activity'), timeLabel(last?.at || agent.updatedAt));
  if (last) latest.append(el('p', '', last.detail || last.kind));
  else latest.append(el('p', 'muted', 'No activity detail reported.'));
  root.append(model, metrics, latest);
  return root;
}
