import { button, el, empty, errorText, field, heading, invariant } from '../../ui/dom.js';
import { icon } from '../../ui/icons.js';
import { approvalCommand, approvalDetails, approvalHeadline, questionCommand } from './model.js';
import { agentLink, connectionNotice, exactBlock, liveView, refreshButton, timeLabel } from './shared.js';

/** @typedef {import('../../types.js').Attention} Attention */
/** @typedef {import('../../types.js').Context} Context */
/** @typedef {{drafts:Map<string,string>,pending:Set<string>,sent:Set<string>,errors:Map<string,string>,render:()=>void,alive:boolean}} Inbox */

/** @param {Context} context @param {Attention} item */
function requireCurrent(context, item) {
  const state = context.store.get();
  invariant(state.role === 'operator', 'Observer access is read only');
  invariant(state.connection === 'online', 'Reconnect before responding');
  const current = state.attention.find(candidate => candidate.id === item.id && candidate.kind === item.kind);
  invariant(current && current.agentId === item.agentId, 'This request is no longer waiting for a response');
  invariant(JSON.stringify(current) === JSON.stringify(item), 'This request changed. Review its latest details before responding');
  return current;
}

/** @param {Attention} item @param {Context} context @param {Inbox} inbox @param {'approved'|'denied'} decision */
async function decide(item, context, inbox, decision) {
  if (inbox.pending.has(item.id) || inbox.sent.has(item.id)) return;
  inbox.pending.add(item.id);
  inbox.errors.delete(item.id);
  inbox.render();
  try {
    requireCurrent(context, item);
    const exact = approvalDetails(item);
    const detail = `Tool: ${exact.tool}\nSide effect: ${exact.effect}\n\nOriginal arguments:\n${exact.argument}\n\nResources:\n${exact.resources}`;
    const accepted = await context.confirm(decision === 'approved' ? 'Approve this exact tool call?' : 'Deny this tool call?', detail);
    if (!accepted || !inbox.alive) return;
    requireCurrent(context, item);
    await context.command(approvalCommand(item, decision));
    inbox.sent.add(item.id);
    context.toast(decision === 'approved' ? 'Approval submitted' : 'Denial submitted');
  } catch (error) {
    inbox.errors.set(item.id, errorText(error));
  } finally {
    inbox.pending.delete(item.id);
    if (inbox.alive) inbox.render();
  }
}

/** @param {Attention} item @param {Context} context @param {Inbox} inbox */
async function reply(item, context, inbox) {
  if (inbox.pending.has(item.id) || inbox.sent.has(item.id)) return;
  inbox.pending.add(item.id);
  inbox.errors.delete(item.id);
  inbox.render();
  try {
    requireCurrent(context, item);
    await context.command(questionCommand(item, inbox.drafts.get(item.id) || ''));
    inbox.sent.add(item.id);
    inbox.drafts.delete(item.id);
    context.store.setDraft(`question:${item.id}`,'');
    context.toast('Reply delivered to this question');
  } catch (error) {
    inbox.errors.set(item.id, errorText(error));
  } finally {
    inbox.pending.delete(item.id);
    if (inbox.alive) inbox.render();
  }
}

/** @param {ReturnType<typeof approvalDetails>} detail */
function approvalPreview(detail) {
  const argument = detail.argument.trimStart();
  if (!argument.startsWith('{') && !argument.startsWith('[')) return detail.argument;
  const fallback = `${detail.tool}(…)`;
  /** @type {unknown} */ let value;
  try { value = JSON.parse(detail.argument); }
  catch (error) {
    if (!(error instanceof SyntaxError)) throw error;
    return fallback;
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return fallback;
  const fields = /** @type {Record<string,unknown>} */ (value);
  for (const key of ['command','cmd','file_path','path','url','query']) {
    const field = fields[key];
    if (Object.hasOwn(fields, key) && typeof field === 'string' && field.trim()) return field;
  }
  return fallback;
}

/** @param {Attention} item @param {Context} context @param {Inbox} inbox @param {boolean} disabled */
function approvalBody(item, context, inbox, disabled) {
  const detail = approvalDetails(item);
  const body = el('div', 'attention-approval');
  const preview = el('div', 'attention-command');
  preview.append(el('span', 'attention-command-label', detail.tool), el('code', '', approvalPreview(detail)));
  body.append(preview);
  if (context.store.get().role === 'operator') {
    body.append(approvalActions(item, context, inbox, disabled));
    if (!disabled) body.append(el('p', 'attention-swipe-help', 'Swipe left to deny · right to approve'));
  }
  const disclosure = el('details', 'fleet-original attention-exact');
  disclosure.dataset.focus = `exact:${item.id}`;
  const summary = el('summary', '', 'Review exact request');
  summary.dataset.focus = `exact-summary:${item.id}`;
  const original = el('details', 'fleet-original attention-raw');
  original.dataset.focus = `original:${item.id}`;
  const rawSummary = el('summary', '', 'Complete approval record');
  rawSummary.dataset.focus = `original-summary:${item.id}`;
  original.append(rawSummary, el('pre', '', detail.original));
  disclosure.append(summary, el('p', 'attention-effect', `Side effect: ${detail.effect}`),
    exactBlock('Original tool arguments', detail.argument), exactBlock('Resources', detail.resources), original);
  body.append(disclosure);
  return body;
}

/** @param {Attention} item @param {Context} context @param {Inbox} inbox @param {boolean} disabled */
function approvalActions(item, context, inbox, disabled) {
  invariant(item.kind === 'approval', 'Decision controls require an approval');
  invariant(context.store.get().role === 'operator', 'Decision controls require operator access');
  const actions = el('div', 'row fleet-actions');
  for (const [label, decision, style] of /** @type {[string,'approved'|'denied',string][]} */ ([
    ['Deny…', 'denied', 'button attention-deny'], ['Approve…', 'approved', 'button primary attention-approve'],
  ])) {
    const action = button(label, () => { void decide(item, context, inbox, decision); }, style);
    action.disabled = disabled;
    action.dataset.focus = `${decision}:${item.id}`;
    actions.append(action);
  }
  return actions;
}

/** @param {Attention} item @param {Context} context @param {Inbox} inbox @param {boolean} disabled */
function questionBody(item, context, inbox, disabled) {
  invariant(item.kind === 'question', 'Reply fields require a question');
  invariant(item.agentId.length > 0, 'Reply fields require an agent');
  const body = el('div', 'stack');
  if (context.store.get().role === 'observer') return body;
  const valid = Number.isSafeInteger(item.questionSeq) && Number(item.questionSeq) > 0;
  if (!valid) {
    body.append(el('p', 'muted', 'The question reference is unavailable. Refresh before replying.'), refreshButton(context));
    return body;
  }
  const form = el('form', 'stack');
  const input = el('textarea', 'fleet-reply');
  input.rows = 2;
  input.required = true;
  input.placeholder = 'Give this agent the context it needs…';
  input.value = inbox.drafts.get(item.id) || '';
  input.disabled = disabled;
  input.dataset.focus = `reply:${item.id}`;
  input.addEventListener('input', () => { inbox.drafts.set(item.id, input.value); context.store.setDraft(`question:${item.id}`,input.value); });
  const submit = el('button', 'button primary', 'Send reply');
  submit.type = 'submit';
  submit.disabled = disabled;
  submit.dataset.focus = `send:${item.id}`;
  form.append(field('Your reply', input), submit);
  form.addEventListener('submit', event => { event.preventDefault(); void reply(item, context, inbox); });
  body.append(form);
  return body;
}

/** @param {Attention} item @param {Context} context @param {Inbox} inbox @param {Map<string,string>} names */
function attentionCard(item, context, inbox, names) {
  invariant(item.id.length > 0, 'Attention item requires an ID');
  invariant(item.agentId.length > 0, 'Attention item requires an agent');
  const state = context.store.get();
  const card = el('article', 'card fleet-attention-card');
  card.dataset.attention = item.id; card.tabIndex = -1;
  card.dataset.kind = item.kind;
  card.append(attentionMetadata(item, context, names.get(item.agentId) || item.agentId));
  card.append(el('h2', '', item.kind === 'approval' ? approvalHeadline(item) : item.title));
  if (item.kind !== 'approval' && item.detail !== item.title) card.append(el('p', 'fleet-request-detail', item.detail));
  const disabled = state.role !== 'operator' || state.connection !== 'online' || inbox.pending.has(item.id) || inbox.sent.has(item.id);
  if (item.kind === 'approval') card.append(approvalBody(item,context,inbox,disabled));
  else if (item.kind === 'question') card.append(questionBody(item,context,inbox,disabled));
  else card.append(button('Review agent',() => context.navigate(`controls/${item.agentId}`)));
  if (item.kind === 'approval' && !disabled) addSwipe(card,decision => { void decide(item,context,inbox,decision); });
  if (inbox.pending.has(item.id) || inbox.sent.has(item.id)) {
    const status = el('p', 'muted', inbox.sent.has(item.id) ? 'Response submitted. Waiting for the latest state.' : 'Response in progress…');
    status.setAttribute('role', 'status');
    card.append(status);
    if (inbox.sent.has(item.id)) card.append(refreshButton(context));
  }
  if (inbox.errors.has(item.id)) {
    const error = el('p', 'fleet-error', inbox.errors.get(item.id));
    error.setAttribute('role', 'alert');
    card.append(error);
  }
  return card;
}

/** @param {Attention} item @param {Context} context @param {string} name */
function attentionMetadata(item, context, name) {
  const labels = {approval:'Approval required',question:'Question for you',agent_failed:'Agent failed',agent_blocked:'Agent blocked'};
  invariant(Object.hasOwn(labels, item.kind), 'Attention kind must have a label');
  invariant(name.length > 0, 'Attention requires an agent name');
  const metadata = el('div', 'fleet-attention-meta');
  const marker = el('span', 'attention-marker');
  marker.append(icon(item.kind === 'approval' ? 'arrow' : 'attention'));
  const identity = el('div', 'attention-identity');
  const link = agentLink(context, item.agentId, name);
  link.dataset.focus = `attention-agent:${item.id}`;
  identity.append(link, el('span', 'attention-kind', labels[item.kind]));
  const time = timeLabel(item.at);
  time.title = time.textContent || '';
  time.setAttribute('aria-label', time.title);
  if (time.dateTime) time.textContent = new Date(item.at).toLocaleTimeString([], {hour:'2-digit', minute:'2-digit'});
  metadata.append(marker, identity, time);
  return metadata;
}

/** @param {HTMLElement} container @param {()=>void} render */
function inboxRedraw(container, render) {
  invariant(container instanceof HTMLElement, 'Inbox redraw requires a container');
  invariant(typeof render === 'function', 'Inbox redraw requires a renderer');
  /** @type {string|undefined} */ let focusKey;
  /** @type {number[]|null} */ let selection = null;
  return () => {
    const active = document.activeElement;
    if (active instanceof HTMLElement && container.contains(active)) {
      focusKey = active.dataset.focus;
      selection = active instanceof HTMLTextAreaElement ? [active.selectionStart, active.selectionEnd] : null;
    }
    const expanded = [...container.querySelectorAll('details[open]')].map(node => /** @type {HTMLElement} */ (node).dataset.focus);
    render();
    for (const node of container.querySelectorAll('details')) node.open = expanded.includes(node.dataset.focus);
    if (!focusKey || (active && active !== document.body && !container.contains(active) && active.isConnected)) return;
    const replacement = [...container.querySelectorAll('[data-focus]')].find(node => /** @type {HTMLElement} */ (node).dataset.focus === focusKey);
    if (!(replacement instanceof HTMLElement)) return;
    replacement.focus({preventScroll:true});
    if (selection && replacement instanceof HTMLTextAreaElement) replacement.setSelectionRange(selection[0],selection[1]);
  };
}

/** @param {HTMLElement} container @param {Context} context */
export function mountAttention(container, context) {
  invariant(container instanceof HTMLElement, 'Attention requires a container');
  invariant(typeof context.store.getDraft === 'function', 'Attention requires persistent drafts');
  let requested = new URLSearchParams(location.hash.split('?')[1] || '').get('item');
  /** @type {Inbox} */
  const inbox = { drafts: new Map(), pending: new Set(), sent: new Set(), errors: new Map(), render: () => {}, alive: true };
  const render = () => {
    const state = context.store.get();
    const approvals = state.attention.filter(item => item.kind === 'approval').length;
    const header = heading('Attention', 'Decisions waiting for you.');
    header.classList.add('attention-heading');
    const counts = el('div', 'row fleet-inbox-counts');
    const questions = state.attention.filter(item => item.kind === 'question').length;
    counts.append(el('span', '', `${approvals} approval${approvals === 1 ? '' : 's'}`), el('span', '', `${questions} question${questions === 1 ? '' : 's'}`));
    const issues = state.attention.length - approvals - questions;
    if (issues) counts.append(el('span','',`${issues} to review`));
    container.replaceChildren(header, counts);
    if (Math.max(approvals,questions,issues) >= 200) container.append(el('p','muted small','The host returns up to 200 items in each category. More may remain after these are resolved.'));
    const notice = connectionNotice(state, context);
    if (notice) container.append(notice);
    if (state.role === 'observer') container.append(el('p', 'muted attention-readonly', 'Observer access · an operator must approve or reply.'));
    const names = new Map(state.agents.map(agent => [agent.id, agent.name || agent.id]));
    const list = el('div', 'stack fleet-inbox');
    for (const item of state.attention) {
      if (!inbox.drafts.has(item.id)) inbox.drafts.set(item.id,context.store.getDraft(`question:${item.id}`));
      list.append(attentionCard(item, context, inbox, names));
    }
    container.append(list);
    const target = requested && [...list.children].find(node => node instanceof HTMLElement && node.dataset.attention === requested);
    if (target instanceof HTMLElement) { requested = null; requestAnimationFrame(() => { target.focus(); target.scrollIntoView({block:'center'}); }); }
    if (!state.attention.length && state.connection === 'online') {
      const cleared = empty('You’re all caught up', 'Approvals and questions will appear here when your agents need you.');
      cleared.append(button('View fleet', () => context.navigate('fleet')));
      container.append(cleared);
    }
  };
  inbox.render = inboxRedraw(container, render);
  const cleanup = liveView(container, context, render);
  return () => { inbox.alive = false; inbox.drafts.clear(); cleanup(); };
}

/** @param {HTMLElement} card @param {(decision:'approved'|'denied')=>void} choose */
function addSwipe(card,choose) {
  invariant(card.dataset.kind === 'approval', 'Only approval cards support decisions by swipe');
  invariant(typeof choose === 'function', 'Swipe must use the confirmed decision handler');
  card.classList.add('attention-swipe');
  let start = {x:0,y:0,pointer:-1,active:false};
  card.addEventListener('pointerdown',event => {
    const interactive = event.target instanceof Element && event.target.closest('button,a,input,textarea,details,code');
    start = {x:event.clientX,y:event.clientY,pointer:event.pointerId,active:event.isPrimary && event.pointerType === 'touch' && !interactive};
  });
  card.addEventListener('pointermove',event => {
    if (!start.active || event.pointerId !== start.pointer) return;
    const dx = event.clientX-start.x; const dy = event.clientY-start.y;
    if (Math.abs(dy) > 30) { start.active = false; delete card.dataset.swipe; return; }
    card.dataset.swipe = Math.abs(dx) > 45 ? dx > 0 ? 'approved' : 'denied' : '';
  });
  card.addEventListener('pointerup',event => {
    const dx = event.clientX-start.x; const dy = event.clientY-start.y;
    if (start.active && event.pointerId === start.pointer && Math.abs(dx)>90 && Math.abs(dy)<30) choose(dx>0 ? 'approved' : 'denied');
    start.active = false;
    delete card.dataset.swipe;
  });
  card.addEventListener('pointercancel',() => { start.active = false; delete card.dataset.swipe; });
}
