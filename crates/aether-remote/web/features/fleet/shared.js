import { button, el, empty, errorText, invariant } from '../../ui/dom.js';

/** @typedef {import('../../types.js').Context} Context */
/** @typedef {import('../../types.js').State} State */

/** @param {HTMLElement} container @param {Context} context @param {()=>void} render */
export function liveView(container, context, render) {
  invariant(container instanceof HTMLElement, 'View requires a container');
  invariant(typeof context.store.subscribe === 'function', 'View requires a reactive store');
  const update = () => {
    const active = document.activeElement;
    const key = active instanceof HTMLElement && container.contains(active) ? active.dataset.focus : undefined;
    const selection = active instanceof HTMLTextAreaElement ? [active.selectionStart, active.selectionEnd] : null;
    const expanded = [...container.querySelectorAll('details[open]')].map(node => /** @type {HTMLElement} */ (node).dataset.focus);
    render();
    for (const node of container.querySelectorAll('details')) {
      if (expanded.includes(node.dataset.focus)) node.open = true;
    }
    if (!key) return;
    const replacement = [...container.querySelectorAll('[data-focus]')].find(node => /** @type {HTMLElement} */ (node).dataset.focus === key);
    if (!(replacement instanceof HTMLElement)) return;
    replacement.focus({ preventScroll: true });
    if (selection && replacement instanceof HTMLTextAreaElement) replacement.setSelectionRange(selection[0], selection[1]);
  };
  update();
  const tracked = () => {
    const state = context.store.get();
    return [state.agents, state.attention, state.activity, state.connection, state.connectionDetail, state.role, state.error];
  };
  let previous = tracked();
  const unsubscribe = context.store.subscribe(() => {
    const next = tracked();
    if (next.every((value, index) => value === previous[index])) return;
    previous = next;
    update();
  });
  return () => { unsubscribe(); container.replaceChildren(); };
}

/** @param {Context} context */
export function refreshButton(context) {
  const node = button('Refresh', () => {
    node.disabled = true;
    void context.store.refresh().catch(error => context.toast(`Refresh failed: ${errorText(error)}`))
      .finally(() => { node.disabled = false; });
  });
  node.dataset.focus = 'refresh';
  return node;
}

/** @param {State} state @param {Context} context @returns {HTMLElement|null} */
export function connectionNotice(state, context) {
  if (state.connection === 'online' && !state.error) return null;
  const loading = state.connection === 'connecting' || state.connection === 'reconnecting';
  const title = state.error ? 'Could not update this view' : loading ? 'Connecting to your fleet' : 'Live updates unavailable';
  const node = empty(title, state.error || state.connectionDetail || 'Connect to your daemon to see the latest state.');
  node.classList.add('fleet-notice');
  node.setAttribute('role', state.error ? 'alert' : 'status');
  if (state.connection === 'unpaired' || state.connection === 'revoked' || state.connection === 'identity-changed') {
    node.append(button('Connect device', () => context.navigate('pair'), 'button primary'));
  } else node.append(refreshButton(context));
  return node;
}

/** @param {string} value */
export function statusBadge(value) {
  const badge = el('span', 'badge fleet-status', value || 'Unknown status');
  badge.dataset.status = value.toLowerCase().replaceAll(' ', '_');
  return badge;
}

/** @param {string} label @param {string} value */
export function metric(label, value) {
  const node = el('div', 'fleet-metric');
  node.append(el('dt', 'muted', label), el('dd', '', value));
  return node;
}

/** @param {string} label @param {string} value */
export function exactBlock(label, value) {
  const section = el('section', 'fleet-exact');
  section.append(el('h3', '', label), el('pre', '', value));
  return section;
}

/** @param {Context} context @param {string} id @param {string} label */
export function agentLink(context, id, label) {
  const link = el('a', 'fleet-agent-link', label);
  link.href = `#agent/${encodeURIComponent(id)}`;
  link.dataset.focus = `agent:${id}`;
  link.addEventListener('click', event => {
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    context.navigate(`agent/${encodeURIComponent(id)}`);
  });
  return link;
}

/** @param {string} value */
export function timeLabel(value) {
  const time = el('time', 'muted');
  const date = new Date(value);
  time.textContent = Number.isNaN(date.getTime()) ? 'Time not reported' : date.toLocaleString();
  if (!Number.isNaN(date.getTime())) time.dateTime = date.toISOString();
  return time;
}
