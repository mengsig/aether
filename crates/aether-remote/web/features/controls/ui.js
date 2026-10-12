import { el, button, field, errorText, invariant } from '../../ui/dom.js';
import { mutate, replyData, records, assertOperator } from './protocol.js';

/** @typedef {import('../../types.js').Context} Context */
/** @typedef {import('../../types.js').Command} Command */
/** @typedef {ReturnType<typeof panel>} Panel */
/** @typedef {{label:string,name:string,value?:string,type?:string,options?:string[],required?:boolean}} InputSpec */

/** @param {string} title @param {string} [detail] */
export function card(title, detail = '') {
  const root = el('section', 'card stack');
  root.append(el('h2', '', title));
  if (detail) root.append(el('p', 'muted', detail));
  return root;
}

/** @param {HTMLElement} container @param {Context} context */
export function panel(container, context) {
  invariant(container instanceof HTMLElement, 'A controls panel needs a container.');
  invariant(typeof context.transport.request === 'function', 'A transport is required.');
  const status = el('p', 'controls-status');
  status.setAttribute('role', 'status');
  const body = el('div', 'stack');
  container.replaceChildren(status, body);
  let alive = true;
  let busy = false;
  const sync = () => {
    const state = context.store.get();
    const locked = busy || state.connection !== 'online';
    for (const node of body.querySelectorAll('[data-mutation], [data-device-self]')) {
      if (node instanceof HTMLButtonElement || node instanceof HTMLFieldSetElement) {
        node.disabled = locked || (node.hasAttribute('data-mutation') && state.role !== 'operator');
      }
    }
  };
  const unsubscribe = context.store.subscribe(sync);
  /** @param {()=>Promise<void>} operation */
  const run = async (operation) => {
    if (!alive || busy) return;
    busy = true;
    status.textContent = 'Working…';
    status.classList.remove('controls-error');
    sync();
    try {
      await operation();
      if (alive && status.textContent === 'Working…') status.textContent = '';
    } catch (error) {
      if (alive) { status.textContent = errorText(error); status.classList.add('controls-error'); }
      else context.toast(errorText(error));
    } finally { busy = false; if (alive) sync(); }
  };
  /** @param {Command} command @param {string} [confirmation] */
  const change = async (command, confirmation) => {
    const result = await mutate(context, command, confirmation);
    if (result && alive) status.textContent = 'Daemon accepted the change.';
    return result;
  };
  return { body, status, context, run, change, sync, alive: () => alive,
    dispose: () => { alive = false; unsubscribe(); } };
}

/** @param {Panel} view @param {string} label @param {()=>Promise<void>} operation @param {boolean|'device'} [mutation] @param {boolean} [danger] */
export function action(view, label, operation, mutation = false, danger = false) {
  const node = button(label, () => { void view.run(operation); }, danger ? 'button danger' : 'button');
  if (mutation === 'device') node.dataset.deviceSelf = 'true';
  else if (mutation) node.dataset.mutation = 'true';
  return node;
}

/** @param {InputSpec} spec */
export function input(spec) {
  const control = spec.options ? el('select') : spec.type === 'textarea' ? el('textarea') : el('input');
  if (control instanceof HTMLSelectElement) {
    for (const value of spec.options || []) {
      const names = { x_high: 'Extra high', queue_one: 'Keep one pending delivery', coalesce: 'Combine into one delivery' };
      const label = names[/** @type {keyof typeof names} */ (value)] || value.charAt(0).toUpperCase()+value.slice(1).replaceAll('_',' ');
      const option = el('option', '', label); option.value = value; control.append(option);
    }
  }
  if (control instanceof HTMLInputElement) control.type = spec.type || 'text';
  if (control instanceof HTMLTextAreaElement) control.rows = 4;
  control.name = spec.name;
  control.value = spec.value || (spec.options?.[0] ?? '');
  control.required = spec.required || false;
  return field(spec.label, control);
}

/** @param {Panel} view @param {string} title @param {InputSpec[]} specs @param {(values:Record<string,string>)=>Promise<void>} submit @param {boolean} [mutation] */
export function form(view, title, specs, submit, mutation = true) {
  invariant(specs.length > 0, 'Forms need at least one field.');
  invariant(new Set(specs.map(spec => spec.name)).size === specs.length, 'Field names must be unique.');
  const root = el('form', 'stack controls-form');
  const fields = el('fieldset', 'stack');
  if (mutation) fields.dataset.mutation = 'true';
  fields.append(el('legend', '', title), ...specs.map(input));
  const send = el('button', 'button primary', title);
  send.type = 'submit';
  fields.append(send);
  root.append(fields);
  root.addEventListener('submit', event => {
    event.preventDefault();
    if (!root.reportValidity()) return;
    const values = Object.fromEntries(Array.from(new FormData(root), ([key, value]) => [key, String(value)]));
    void view.run(async () => {
      if (mutation) assertOperator(view.context);
      await submit(values);
    });
  });
  return root;
}

/** @param {Panel} view @param {Command} command @param {string} expected */
export async function readList(view, command, expected) {
  const reply = await view.context.transport.request(command);
  return records(replyData(reply, expected));
}

/** @param {unknown} value */
export function display(value) {
  if (value === undefined || value === null) return 'Not reported';
  if (typeof value === 'object') return JSON.stringify(value, null, 2);
  return String(value);
}

/** @param {Record<string,unknown>} data @param {string[]} [keys] */
export function details(data, keys = Object.keys(data)) {
  const list = el('dl', 'controls-details');
  for (const key of keys) {
    list.append(el('dt', '', key.replace(/([a-z])([A-Z])/g, '$1 $2').replaceAll('_', ' ')), el('dd', '', display(data[key])));
  }
  return list;
}

/** @param {HTMLElement} target @param {HTMLElement[]} rows @param {string} message */
export function replaceRows(target, rows, message) {
  target.replaceChildren(...(rows.length ? rows : [el('p', 'empty-state', message)]));
}
