import {button, el, errorText, invariant} from '../../ui/dom.js';
import {searchFiles} from '../../core/contract.js';
import {mentionAt, replaceMention} from './state.js';
import {chatButton} from './chrome.js';

/** @typedef {{root:HTMLElement,list:HTMLElement,generation:number,timer:number,disposed:boolean}} MentionState */

/** @param {HTMLTextAreaElement} input @param {import('../../types.js').Context} context @param {string} agentId @param {()=>void} persist */
export function mentions(input, context, agentId, persist) {
  invariant(input instanceof HTMLTextAreaElement, 'Mentions require a draft');
  invariant(agentId.length > 0, 'Mentions require a host agent');
  const root = el('div', 'conversation-mentions');
  root.hidden = true;
  root.setAttribute('aria-label', 'File and agent mentions');
  root.setAttribute('role', 'dialog');
  const list = el('div', 'conversation-mention-list');
  const state = {root, list, generation:0, timer:0, disposed:false};
  const close = () => { state.generation++; clearTimeout(state.timer); root.hidden = true; };
  const heading = el('div', 'conversation-sheet-heading');
  heading.append(el('h2', '', 'Files & agents'), chatButton('close', 'Close mentions', () => { close(); input.focus(); }));
  root.append(heading, list);
  const keyboard = (/** @type {KeyboardEvent} */ event) => {
    if (event.key !== 'ArrowDown' || root.hidden) return;
    const first = list.querySelector('button');
    if (first) { event.preventDefault(); first.focus(); }
  };
  root.addEventListener('keydown', event => { if (event.key === 'Escape') { close(); input.focus(); } });
  input.addEventListener('keydown', keyboard);
  return {
    root,
    update() {
      close();
      if (input.readOnly || !mentionAt(input.value, input.selectionStart)) return;
      const generation = state.generation;
      state.timer = window.setTimeout(() => { void lookup(state, generation, input, context, agentId, persist); }, 220);
    },
    close,
    destroy() { state.disposed = true; close(); input.removeEventListener('keydown', keyboard); },
  };
}

/** @param {MentionState} state @param {number} generation @param {HTMLTextAreaElement} input @param {import('../../types.js').Context} context @param {string} agentId @param {()=>void} persist */
async function lookup(state, generation, input, context, agentId, persist) {
  const mention = mentionAt(input.value, input.selectionStart);
  if (!mention || state.disposed || generation !== state.generation || input.readOnly) return;
  state.root.hidden = false;
  state.list.replaceChildren(el('p', 'muted', 'Searching host files…'));
  const agents = context.store.get().agents.filter(agent => `${agent.name} ${agent.id}`.toLowerCase().includes(mention.query.toLowerCase())).slice(0, 6);
  const workspace = context.store.get().agents.find(agent => agent.id === agentId)?.workspace;
  const choose = (/** @type {string} */ path) => {
    const current = mentionAt(input.value, input.selectionStart);
    if (input.readOnly || !current || current.query !== mention.query) return;
    const replacement = replaceMention(input.value, current, path);
    input.value = replacement.text;
    input.setSelectionRange(replacement.cursor, replacement.cursor);
    state.root.hidden = true;
    persist(); input.focus();
  };
  try {
    if (!workspace) throw new Error('Agent workspace is not available for file search.');
    const {entries:files,truncated} = await searchFiles(context.transport, workspace, mention.query);
    if (generation !== state.generation || state.disposed) return;
    state.list.replaceChildren();
    for (const file of files.slice(0, 30)) state.list.append(button(`${file.isDirectory ? 'Folder' : 'File'} · ${file.path}`, () => choose(file.path)));
    if (truncated || files.length > 30) state.list.append(el('p','muted','More files match. Refine your mention to narrow results.'));
    if (!files.length && !agents.length) state.list.append(el('p', 'muted', 'No matching files or agents'));
  } catch (error) {
    if (generation !== state.generation || state.disposed) return;
    state.list.replaceChildren(el('p', 'muted', `File search unavailable: ${errorText(error)}`));
  }
  for (const agent of agents) state.list.append(button(`Agent · ${agent.name}`, () => choose(agent.id)));
}
