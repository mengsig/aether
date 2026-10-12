import { el, button, heading, errorText, invariant } from '../../ui/dom.js';
import { required } from './protocol.js';
import { mountAgent } from './agent.js';
import { mountSchedules } from './schedules.js';
import { mountMemories } from './memories.js';
import { mountArtifacts, mountRuns, mountDiagnostics } from './resources.js';
import { mountRules } from './rules.js';
import { applyIntent } from './intent.js';
export { mountSettings } from './settings.js';

/** @param {HTMLElement} container @param {import('../../types.js').Context} context @param {string} agentId */
export function mountControls(container, context, agentId) {
  required(agentId, 'Agent ID');
  invariant(container instanceof HTMLElement, 'Controls require a container.');
  const agent = context.store.get().agents.find(item => item.id === agentId);
  const root = el('div', 'stack controls-screen');
  const tabs = el('nav', 'row controls-tabs');
  tabs.setAttribute('aria-label', 'Agent controls sections');
  const content = el('div');
  const role = el('p', 'muted');
  const sync = () => {
    role.hidden = context.store.get().role === 'operator';
    role.textContent = 'Observer access · changes are disabled.';
  };
  /** @type {Record<string,[string,(container:HTMLElement,context:import('../../types.js').Context,agent:string)=>()=>void]>} */
  const sections = { agent: ['Agent', mountAgent], schedules: ['Schedules', mountSchedules], memories: ['Memories', mountMemories],
    artifacts: ['Artifacts', mountArtifacts], runs: ['Runs & reports', mountRuns], diagnostics: ['Diagnostics', mountDiagnostics], rules: ['Approval rules', mountRules] };
  let cleanup = () => {};
  /** @param {string} key */
  const select = key => {
    cleanup();
    cleanup = () => {};
    for (const tab of tabs.querySelectorAll('button')) tab.setAttribute('aria-pressed', String(tab.dataset.panel === key));
    try { cleanup = sections[key][1](content, context, agentId); }
    catch (error) { content.replaceChildren(el('p', 'controls-error', errorText(error))); }
  };
  for (const [key, [label]] of Object.entries(sections)) {
    const tab = button(label, () => select(key));
    tab.dataset.panel = key;
    tabs.append(tab);
  }
  root.append(heading('Agent controls', agent?.name || agentId), role, tabs, content);
  container.replaceChildren(root);
  const requested = new URLSearchParams(location.hash.split('?')[1] || '').get('panel') || 'agent';
  select(Object.hasOwn(sections, requested) ? requested : 'agent');
  applyIntent(content,new URLSearchParams(location.hash.split('?')[1] || ''));
  sync();
  const unsubscribe = context.store.subscribe(sync);
  return () => { cleanup(); unsubscribe(); };
}
