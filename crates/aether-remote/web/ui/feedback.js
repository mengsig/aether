import {el, button, invariant} from './dom.js';

/** @param {HTMLElement} host */
export function createToast(host) {
  invariant(host instanceof HTMLElement, 'Feedback requires a DOM host');
  const region = el('div', 'toast');
  region.setAttribute('role', 'status'); region.hidden = true;
  host.append(region);
  let timer = 0;
  function restore() {
    if (region.hidden) return;
    const conversation = document.querySelector('.conversation-screen');
    const main = document.querySelector('main');
    if (conversation) conversation.querySelector('.conversation-header')?.after(region);
    else if (main) main.prepend(region);
  }
  /** @param {string} message */
  function show(message) {
    invariant(typeof message === 'string', 'Feedback must be plain text');
    clearTimeout(timer); region.hidden = false; region.textContent = message;
    restore();
    timer = window.setTimeout(() => { region.hidden = true; }, 6500);
  }
  return Object.assign(show,{restore});
}

/** @param {HTMLElement} host */
export function createConfirm(host) {
  return /** @param {string} title @param {string} detail */ (title, detail) => new Promise(resolve => {
    const previous = document.activeElement;
    const dialog = el('dialog', 'confirm-dialog');
    const heading = el('h2', '', title); heading.id = 'confirmation-title';
    const description = el('p', '', detail); description.id = 'confirmation-detail';
    dialog.setAttribute('aria-labelledby', heading.id);
    dialog.setAttribute('aria-describedby', description.id);
    const controls = el('div', 'row');
    const cancel = button('Keep as is', () => dialog.close('cancel'));
    controls.append(cancel, button('Confirm', () => dialog.close('confirm'), 'button danger'));
    dialog.append(heading, description, controls); host.append(dialog);
    dialog.addEventListener('close', () => {
      const confirmed = dialog.returnValue === 'confirm'; dialog.remove();
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
      resolve(confirmed);
    }, {once:true});
    dialog.showModal(); cancel.focus();
  });
}
