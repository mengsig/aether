import {el, invariant} from '../../ui/dom.js';
import {searchEntries} from './state.js';
import {chatButton} from './chrome.js';

/** @param {(index:number,query:string)=>void} go @param {()=>void} dismiss */
export function transcriptSearch(go, dismiss) {
  invariant(typeof go === 'function', 'Search requires transcript navigation');
  invariant(typeof dismiss === 'function', 'Search requires a focus return target');
  const root = el('div', 'conversation-search row');
  root.hidden = true;
  root.setAttribute('role', 'search');
  const input = el('input');
  input.type = 'search';
  input.placeholder = 'Search conversation';
  input.setAttribute('aria-label', 'Search full conversation');
  const status = el('span', 'muted');
  status.setAttribute('role', 'status');
  const state = {entries:/** @type {import('../../types.js').TranscriptEntry[]} */([]), matches:/** @type {number[]} */([]), current:0, timer:0};
  const previous = chatButton('previous', 'Previous search match', () => move(-1));
  const next = chatButton('next', 'Next search match', () => move(1));
  const close = () => { clearTimeout(state.timer); root.hidden = true; dismiss(); };
  root.append(input, previous, next, chatButton('close', 'Close conversation search', close), status);
  function display() {
    previous.disabled = next.disabled = state.matches.length === 0;
    status.textContent = !input.value.trim() ? '' : state.matches.length ? `${state.current + 1} of ${state.matches.length}` : 'No matches';
  }
  function search() {
    state.matches = searchEntries(state.entries, input.value);
    state.current = 0;
    display();
    if (state.matches.length) go(state.matches[0], input.value);
  }
  /** @param {number} delta */
  function move(delta) {
    if (!state.matches.length) return;
    state.current = (state.current + delta + state.matches.length) % state.matches.length;
    display();
    go(state.matches[state.current], input.value);
  }
  input.addEventListener('input', () => { clearTimeout(state.timer); state.timer = window.setTimeout(search, 150); });
  input.addEventListener('keydown', event => {
    if (event.key === 'Enter') { event.preventDefault(); move(event.shiftKey ? -1 : 1); }
    if (event.key === 'Escape') close();
  });
  const focus = () => { root.hidden = false; input.focus(); };
  const shortcut = (/** @type {KeyboardEvent} */ event) => {
    if ((event.ctrlKey || event.metaKey) && event.key === 'f') { event.preventDefault(); focus(); }
  };
  document.addEventListener('keydown', shortcut);
  display();
  return {
    root, focus,
    /** @param {import('../../types.js').TranscriptEntry[]} entries */
    set(entries) {
      if (entries === state.entries) return;
      state.entries = entries;
      state.matches = searchEntries(entries, input.value);
      state.current = Math.min(state.current, Math.max(0, state.matches.length - 1));
      display();
    },
    destroy() { clearTimeout(state.timer); document.removeEventListener('keydown', shortcut); },
  };
}
