import {button, el, invariant} from '../../ui/dom.js';
import {findCommands} from './commands.js';
import {chatButton} from './chrome.js';

/** @param {(command:import('./commands.js').SlashCommand, argument:string)=>void} execute @param {()=>void} dismiss */
export function commandPalette(execute, dismiss) {
  invariant(typeof execute === 'function', 'Commands require an executor');
  invariant(typeof dismiss === 'function', 'Commands require a focus return target');
  const root = el('div', 'conversation-palette');
  root.hidden = true;
  root.setAttribute('aria-label', 'Commands');
  root.setAttribute('role', 'dialog');
  const heading = el('div', 'conversation-sheet-heading');
  const close = () => { root.hidden = true; dismiss(); };
  heading.append(el('h2', '', 'Commands'), chatButton('close', 'Close commands', close));
  const filter = el('input');
  filter.type = 'search';
  filter.placeholder = 'Find a command…';
  filter.setAttribute('aria-label', 'Filter commands');
  const list = el('div', 'conversation-command-list');
  root.append(heading, filter, list);
  function render() {
    list.replaceChildren();
    for (const command of findCommands(filter.value)) {
      const item = button('', () => { root.hidden = true; execute(command, ''); }, 'button conversation-command');
      item.append(el('strong', '', `/${command.name}`), el('span', 'muted', command.description));
      list.append(item);
    }
    if (!list.childElementCount) list.append(el('p', 'muted', 'No matching commands'));
  }
  filter.addEventListener('input', render);
  root.addEventListener('keydown', event => {
    if (event.key === 'Escape') close();
    if (event.key === 'ArrowDown' && event.target === filter) {
      event.preventDefault();
      list.querySelector('button')?.focus();
    }
  });
  return {
    root,
    /** @param {string} [query] @param {boolean} [focus] */
    open(query = '', focus = true) {
      root.hidden = false;
      filter.value = query;
      render();
      if (focus) filter.focus();
    },
    close() { root.hidden = true; },
  };
}
