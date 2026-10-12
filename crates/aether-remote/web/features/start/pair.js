import {el, button, field, errorText} from '../../ui/dom.js';
import {normalizeDeviceName} from '../../core/contract.js';

export function takePairingSecret() {
  const hash = location.hash.slice(1);
  const values = new URLSearchParams(hash);
  const secret = values.get('pair') || values.get('code') || values.get('secret') || '';
  if (secret) history.replaceState(null, '', `${location.pathname}${location.search}#pair`);
  return secret;
}

/** @param {HTMLElement} container @param {import('../../types.js').Context} context @param {string} secret */
export function mountPair(container, context, secret) {
  const intro = el('header', 'pair-intro');
  intro.append(el('p','eyebrow','Aether, wherever you are'),el('h1','','Your agents. Within reach.'),
    el('p','muted','Follow the work. Make the decisions. Keep everything moving, from here.'));
  const panel = el('section','card stack');
  const form = el('form','stack');
  const name = el('input'); name.value = /iPhone|iPad/.test(navigator.userAgent) ? 'My iPhone or iPad' : 'My device';
  name.required = true; name.autocomplete = 'off';
  name.addEventListener('input',()=>{
    try { normalizeDeviceName(name.value); name.setCustomValidity(''); }
    catch (cause) { name.setCustomValidity(errorText(cause)); }
  });
  const code = el('input'); code.value = secret; code.required = true; code.autocomplete = 'off';
  code.autocapitalize = 'characters'; code.spellcheck = false; code.placeholder = 'ABCD-1234';
  const submit = el('button','button primary','Pair this device'); submit.type = 'submit';
  const error = el('p','error-banner'); error.hidden = true; error.setAttribute('role','alert');
  form.append(field('Device name',name));
  if (!secret) form.append(field('Pairing code',code), el('p','muted small','Type the code your computer is showing. Run “aether remote pair” there for a new one.'));
  const progress = el('div','notice'); progress.hidden = true; progress.setAttribute('role','status');
  form.append(submit,progress,error);
  const installFirst = isIOS() && !isInstalled();
  panel.append(el('h2','',installFirst ? 'Or pair in Safari' : 'Connect securely'),
    el('p','muted small',installFirst ? 'This works in Safari only: no notifications, and the Home Screen app would need its own pairing.'
      : 'A private key stays on this device. Your daemon decides what this device can access.'),form);
  if (installFirst) { panel.style.marginTop = '16px'; container.append(intro,installFirstGuide(secret),panel); }
  else container.append(intro,panel,...(isInstalled() ? [] : [installGuide()]));
  let disposed = false;
  form.addEventListener('submit', async event => {
    event.preventDefault(); if (!form.reportValidity()) return;
    submit.disabled = true; error.hidden = true;
    try {
      await context.transport.pair(code.value.trim(),normalizeDeviceName(name.value), state => {
        if (disposed) return;
        progress.hidden = false; progress.replaceChildren(el('h3','','Confirm on your host'),
          el('p','',state.phrase),el('p','small','Check that this phrase matches the host, then approve this device there.'),
          el('p','small muted',`Fingerprint: ${state.fingerprint}`));
      });
      code.value = ''; secret = ''; context.toast('Device paired. You’re connected.'); context.navigate('fleet');
    } catch (cause) { if (!disposed) { error.textContent = errorText(cause); error.hidden = false; } }
    finally { if (!disposed) submit.disabled = false; }
  });
  return () => { disposed = true; secret = ''; code.value = ''; };
}

function isIOS() {
  return /iPad|iPhone|iPod/.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
}

function isInstalled() {
  return ('standalone' in navigator && navigator.standalone === true) || matchMedia('(display-mode: standalone)').matches;
}

// iOS gives a Home Screen app its own storage, so a pairing made in Safari never reaches it.
/** @param {string} secret */
function installFirstGuide(secret) {
  const section = el('section','card stack');
  const steps = el('ol','pair-steps');
  const last = el('li','small',secret ? 'Enter this pairing code there:' : 'Enter the pairing code from your computer there.');
  if (secret) { const code = el('strong','pair-code',secret); code.setAttribute('aria-label',`Pairing code ${secret.split('').join(' ')}`); last.append(code); }
  steps.append(el('li','small','Tap Share, then Add to Home Screen.'),el('li','small','Open Aether from the new Home Screen icon.'),last);
  section.append(el('h2','','Add Aether to your Home Screen first'),
    el('p','muted small','On iPhone and iPad the Home Screen app keeps its own pairing, and only it can send notifications.'),steps);
  return section;
}

export function installGuide() {
  const section = el('section','stack'); section.style.marginTop = '32px';
  const steps = el('ol','pair-steps');
  for (const text of [
    isIOS() ? 'In Safari, tap Share, then Add to Home Screen.' : 'Open your browser menu, then choose Install app or Add to Home Screen.',
    'Open Aether from your Home Screen for a focused, full-screen experience.',
    'Enable notifications in Settings when you’re ready to receive decisions and updates.',
  ]) steps.append(el('li','small',text));
  section.append(el('h2','','Make it feel at home'),steps);
  return section;
}
