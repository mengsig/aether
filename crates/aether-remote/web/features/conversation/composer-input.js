import {button, el, errorText, invariant} from '../../ui/dom.js';
import {chatButton} from './chrome.js';

/** @typedef {import('../../types.js').Context} Context */
/** @typedef {ReturnType<typeof draftView> & {busy:boolean,disposed:boolean}} ComposerState */

/** @param {Context} context @param {string} agentId @param {()=>void} submit */
export function draftView(context, agentId, submit) {
  invariant(agentId.length > 0, 'Draft requires an agent');
  invariant(typeof context.store.getDraft === 'function', 'Draft requires persistence');
  const root = el('form', 'conversation-composer');
  const pill = el('div', 'conversation-compose-pill');
  const input = el('textarea', 'conversation-draft');
  input.rows = 1;
  input.placeholder = 'Message…';
  input.setAttribute('aria-label', 'Message draft');
  input.value = context.store.getDraft(agentId);
  const hint = el('p', 'muted conversation-compose-hint');
  const status = el('p', 'muted conversation-compose-status');
  status.setAttribute('role', 'status');
  const send = chatButton('send', 'Send', submit);
  send.classList.add('conversation-send');
  const restart = button('Restart to continue', () => context.navigate(`controls/${agentId}`), 'button quiet');
  restart.hidden = true;
  root.append(pill, hint, restart, status);
  return {root, pill, input, hint, status, send, restart};
}

/** @param {HTMLTextAreaElement} input */
export function resizeInput(input) {
  invariant(input instanceof HTMLTextAreaElement, 'Draft resizing requires a text area');
  invariant(input.rows > 0, 'Draft must retain an editable row');
  if (!input.isConnected || input.hidden) return;
  input.style.height = '0px';
  input.style.height = `${input.scrollHeight}px`;
}

/** @param {ComposerState} state @param {Context} context @param {string} agentId */
export function saveDraft(state, context, agentId) {
  invariant(agentId.length > 0, 'Draft persistence requires its owner');
  invariant(typeof state.input.value === 'string', 'Draft persistence requires text');
  if (context.store.get().role !== 'operator') return false;
  try { context.store.setDraft(agentId, state.input.value); return true; }
  catch (error) { state.status.dataset.tone = 'error'; state.status.textContent = `Draft could not be saved: ${errorText(error)}`; return false; }
}

/** @param {import('../../types.js').State} snapshot @param {string} agentId */
export function finishedAgent(snapshot, agentId) {
  return ['completed', 'failed', 'cancelled'].includes(snapshot.agents.find(agent => agent.id === agentId)?.status ?? '');
}

/** @param {ComposerState} state @param {Context} context @param {string} agentId */
export function updateRole(state, context, agentId) {
  const snapshot = context.store.get();
  invariant(snapshot.role === 'operator' || snapshot.role === 'observer', 'Composer role must be normalized');
  invariant(state.input instanceof HTMLTextAreaElement, 'Composer access requires a draft input');
  const restoring = snapshot.ready === false;
  const observer = snapshot.role !== 'operator' && !restoring;
  const finished = finishedAgent(snapshot, agentId);
  state.input.readOnly = observer || finished || restoring;
  state.input.hidden = state.send.hidden = observer;
  state.send.disabled = restoring || observer || finished || state.busy || !state.input.value.trim() || snapshot.connection !== 'online';
  state.root.dataset.readonly = String(state.input.readOnly);
  state.restart.hidden = restoring || observer || !finished;
  state.hint.textContent = observer ? 'Observer access · Read, search, and copy.' : finished ? 'This agent has finished. Your draft is saved.' : '';
}

/** @param {HTMLTextAreaElement} input @param {()=>void} update */
export function insertMention(input, update) {
  invariant(input instanceof HTMLTextAreaElement, 'Mention requires a draft');
  invariant(input.selectionStart <= input.selectionEnd, 'Draft selection must be ordered');
  if (input.readOnly) return;
  const before = input.value.slice(0, input.selectionStart);
  input.setRangeText(`${before && !/\s$/.test(before) ? ' ' : ''}@`, input.selectionStart, input.selectionEnd, 'end');
  input.focus(); update();
}
