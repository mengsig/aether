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
  const code = el('input'); code.type = 'password'; code.value = secret; code.required = true; code.autocomplete = 'off';
  const submit = el('button','button primary','Pair this device'); submit.type = 'submit';
  const error = el('p','error-banner'); error.hidden = true; error.setAttribute('role','alert');
  form.append(field('Device name',name));
  if (!secret) form.append(field('Pairing secret',code), el('p','muted small','Open the pairing link or scan the QR code provided by your host operator.'));
  const progress = el('div','notice'); progress.hidden = true; progress.setAttribute('role','status');
  form.append(submit,progress,error);
  panel.append(el('h2','','Connect securely'),el('p','muted small','A private key stays on this device. Your daemon decides what this device can access.'),form);
  container.append(intro,panel,installGuide());
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
      if (disposed) return;
      code.value = ''; secret = ''; context.toast('Device paired. You’re connected.'); context.navigate('fleet');
    } catch (cause) { if (!disposed) { error.textContent = errorText(cause); error.hidden = false; } }
    finally { if (!disposed) submit.disabled = false; }
  });
  return () => { disposed = true; secret = ''; code.value = ''; };
}

export function installGuide() {
  const section = el('section','stack'); section.style.marginTop = '32px';
  const steps = el('ol','pair-steps');
  const ios = /iPad|iPhone|iPod/.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
  for (const text of [
    ios ? 'In Safari, tap Share, then Add to Home Screen.' : 'Open your browser menu, then choose Install app or Add to Home Screen.',
    'Open Aether from your Home Screen for a focused, full-screen experience.',
    'Enable notifications in Settings when you’re ready to receive decisions and updates.',
  ]) steps.append(el('li','small',text));
  section.append(el('h2','','Make it feel at home'),steps);
  return section;
}
