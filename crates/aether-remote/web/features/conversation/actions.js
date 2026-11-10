import {button, el, invariant} from '../../ui/dom.js';
import {copyText} from './markdown.js';
import {chatButton} from './chrome.js';

/** @param {HTMLElement} node @param {string} text @param {import('../../types.js').Context} context @param {(text:string)=>void} quote */
export function messageActions(node, text, context, quote) {
  invariant(node instanceof HTMLElement, 'Message actions require a transcript row');
  invariant(typeof text === 'string', 'Message actions require original message text');
  const open = () => openActions(node, text, context, quote);
  const more = chatButton('more', 'Copy, quote, or select this message', open);
  more.setAttribute('aria-haspopup', 'dialog');
  more.classList.add('conversation-more');
  node.append(more);
  let timer = 0;
  let x = 0;
  let y = 0;
  const cancel = () => { clearTimeout(timer); timer = 0; };
  node.addEventListener('pointerdown', event => {
    if (event.pointerType !== 'touch' || /** @type {Element} */(event.target).closest('button,a,summary')) return;
    x = event.clientX;
    y = event.clientY;
    timer = window.setTimeout(open, 600);
  });
  node.addEventListener('pointermove', event => {
    if (Math.abs(event.clientX - x) + Math.abs(event.clientY - y) > 12) cancel();
  });
  node.addEventListener('pointerup', cancel);
  node.addEventListener('pointercancel', cancel);
  node.addEventListener('pointerleave', cancel);
  node.addEventListener('click', event => {
    if (/** @type {Element} */(event.target).closest('button,a,summary,details,pre,code,dialog')) return;
    if (window.getSelection()?.toString()) return;
    node.classList.toggle('conversation-actions-visible');
  });
  node.addEventListener('contextmenu', event => {
    if (/** @type {Element} */(event.target).closest('a,pre,code')) return;
    event.preventDefault();
    open();
  });
}

/** @param {HTMLElement} node @param {string} text @param {import('../../types.js').Context} context @param {(text:string)=>void} quote */
function openActions(node, text, context, quote) {
  invariant(node instanceof HTMLElement, 'Action sheet requires a message');
  invariant(typeof quote === 'function', 'Action sheet requires a quote handler');
  if (!node.isConnected || node.querySelector('dialog[open]')) return;
  const dialog = el('dialog', 'conversation-actions stack');
  dialog.setAttribute('aria-label', 'Message actions');
  const close = () => { dialog.close(); dialog.remove(); };
  dialog.append(el('h2', '', 'Message actions'));
  dialog.append(button('Copy message', () => { void copyText(text, context.toast); close(); }));
  if (context.store.get().role === 'operator') {
    dialog.append(button('Quote in draft', () => { quote(text); close(); }));
  }
  dialog.append(button('Select text', () => {
    close();
    const range = document.createRange();
    range.selectNodeContents(node.querySelector('.conversation-content') ?? node);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
  }));
  dialog.append(button('Message details', () => {
    close();
    const details = node.querySelector('.conversation-provenance');
    invariant(details instanceof HTMLDetailsElement, 'Message details must exist');
    details.hidden = false; details.open = true;
    details.querySelector('summary')?.focus();
  }));
  dialog.append(button('Close', close));
  dialog.addEventListener('close', () => dialog.remove(), {once:true});
  node.append(dialog);
  dialog.showModal();
}
