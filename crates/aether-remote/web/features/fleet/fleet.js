import { button, el, empty, heading, invariant } from '../../ui/dom.js';
import { connectionNotice } from './shared.js';
import { createGraph } from './graph/canvas.js';
import { agentState, PAGE_SIZE } from './graph/layout.js';
import { compactList } from './graph/list.js';
import { createAgentSheet } from './graph/sheet.js';

/** @typedef {import('../../types.js').Context} Context */
/** @typedef {import('../../types.js').State} State */

/** @param {State} state */
function fleetSummary(state) {
  invariant(Array.isArray(state.agents), 'Fleet summary requires agents');
  const counts = { running: 0, waiting: 0, done: 0, failed: 0, ready: 0 };
  for (const agent of state.agents) counts[agentState(agent)]++;
  invariant(Object.values(counts).reduce((sum, count) => sum + count, 0) === state.agents.length, 'Fleet states must cover every agent');
  const summary = el('dl', 'fleet-summary');
  for (const status of ['running', 'waiting', 'done', ...(counts.failed ? ['failed'] : [])]) {
    const item = el('div'); item.dataset.state = status;
    item.append(el('dt', '', status), el('dd', '', String(counts[/** @type {keyof typeof counts} */ (status)])));
    summary.append(item);
  }
  summary.setAttribute('aria-label', `${state.agents.length} agents`);
  return summary;
}

/** @param {Context} context @param {(tree:boolean)=>void} select */
function fleetFrame(context, select) {
  const root = el('div', 'fleet-view'); const header = heading('Your fleet');
  header.classList.add('fleet-heading');
  const summary = el('div'); const toolbar = el('div', 'fleet-toolbar');
  const switcher = el('div', 'fleet-switcher'); switcher.setAttribute('role', 'group'); switcher.setAttribute('aria-label', 'Fleet layout');
  for (const [label, value] of /** @type {[string,boolean][]} */ ([['Tree', true], ['List', false]])) {
    const toggle = button(label, () => select(value)); toggle.setAttribute('aria-pressed', String(value)); switcher.append(toggle);
  }
  const start = button('Start work', () => context.navigate('new'), 'button fleet-start');
  toolbar.append(switcher, start);
  const notices = el('div'); const body = el('div'); const paging = el('div', 'fleet-pagination');
  root.append(header, summary, toolbar, notices, body, paging);
  return { root, summary, switcher, start, notices, body, paging };
}

/** @param {HTMLElement} container @param {Context} context */
export function mountFleet(container, context) {
  invariant(container instanceof HTMLElement, 'Fleet requires a container');
  invariant(typeof context.navigate === 'function', 'Fleet requires agent navigation');
  let tree = true; let page = 0;
  const frame = fleetFrame(context, value => {
    tree = value; render();
    const selected = frame.switcher.querySelector('[aria-pressed="true"]');
    if (selected instanceof HTMLElement) selected.focus();
  });
  container.replaceChildren(frame.root);
  const sheet = createAgentSheet(frame.root, context);
  const graph = createGraph(sheet.open);
  const render = () => {
    const state = context.store.get();
    page = Math.min(page, Math.max(0, Math.ceil(state.agents.length / PAGE_SIZE) - 1));
    frame.summary.replaceChildren(...(!state.agents.length && state.ready === false ? [] : [fleetSummary(state)])); frame.start.hidden = state.role !== 'operator';
    for (const toggle of frame.switcher.querySelectorAll('button')) toggle.setAttribute('aria-pressed', String((toggle.textContent === 'Tree') === tree));
    frame.notices.replaceChildren(); const notice = connectionNotice(state, context); if (notice) frame.notices.append(notice);
    if (state.ready && state.role === 'observer') frame.notices.append(el('p', 'fleet-observer muted', 'Observer access · live view, read only'));
    if (!state.agents.length) frame.body.replaceChildren(state.ready === false || ['connecting', 'reconnecting'].includes(state.connection) ? fleetSkeleton() : empty('A quiet fleet', 'New agents will appear here as work begins.'));
    else if (tree) { if (!frame.body.contains(graph.root)) frame.body.replaceChildren(graph.root); graph.update(state, page); }
    else {
      const focus = document.activeElement instanceof HTMLElement ? document.activeElement.dataset.focus : undefined;
      frame.body.replaceChildren(compactList(state, context, page, sheet.open));
      for (const link of frame.body.querySelectorAll('a')) if (focus && link.dataset.focus === focus) link.focus({ preventScroll: true });
    }
    updatePaging(frame.paging, state.agents.length, page, value => { page = value; render(); const first = frame.paging.querySelector('button'); if (first instanceof HTMLButtonElement) first.focus(); });
    sheet.update();
  };
  render();
  let previous = context.store.get();
  const unsubscribe = context.store.subscribe(() => {
    const state = context.store.get();
    if (state.agents === previous.agents && state.attention === previous.attention && state.activity === previous.activity && state.connection === previous.connection && state.connectionDetail === previous.connectionDetail && state.error === previous.error && state.role === previous.role && state.ready === previous.ready) return;
    previous = state; render();
  });
  const timer = window.setInterval(() => sheet.update(), 60000);
  return () => { window.clearInterval(timer); unsubscribe(); sheet.dispose(); graph.dispose(); container.replaceChildren(); };
}

/** @param {HTMLElement} host @param {number} total @param {number} page @param {(page:number)=>void} change */
function updatePaging(host, total, page, change) {
  host.replaceChildren(); host.hidden = total <= PAGE_SIZE;
  if (host.hidden) return;
  invariant(page * PAGE_SIZE < total, 'Fleet page must contain agents');
  invariant(Number.isSafeInteger(total), 'Agent count must be a safe integer');
  const previous = button('Previous', () => change(page - 1)); previous.disabled = page === 0;
  const next = button('Next', () => change(page + 1)); next.disabled = (page + 1) * PAGE_SIZE >= total;
  host.append(el('p', 'muted', `${page * PAGE_SIZE + 1}–${Math.min(total, (page + 1) * PAGE_SIZE)} of ${total} agents`), previous, next);
}

function fleetSkeleton() {
  const root = el('div', 'fleet-graph-skeleton'); root.setAttribute('role', 'status'); root.setAttribute('aria-label', 'Loading your fleet');
  for (let index = 0; index < 4; index++) { const node = el('span'); node.setAttribute('aria-hidden', 'true'); root.append(node); }
  root.append(el('p', 'muted', 'Loading your fleet…'));
  return root;
}
