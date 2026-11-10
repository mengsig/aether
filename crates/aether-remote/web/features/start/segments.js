import {el,button,invariant,errorText} from '../../ui/dom.js';
import {modelOptions} from './models.js';

/** @typedef {{id:string,label:string,disabled?:boolean}} Choice */
/** @param {string} title @param {string} name @param {Choice[]} choices @param {string} initial @param {(value:string)=>void} [changed] */
export function segments(title,name,choices,initial,changed = () => {}) {
  const container = el('fieldset','start-choice');
  const legend = el('legend','field-label',title); const row = el('div','start-segments');
  container.append(legend,row); let value = initial;
  /** @param {Choice[]} items @param {string} selected */
  const replace = (items,selected) => {
    invariant(new Set(items.map(item => item.id)).size === items.length,'Duplicate choice identity');
    invariant(!selected || items.some(item => item.id === selected && !item.disabled),'Unavailable choice');
    const focused = row.contains(document.activeElement);
    value = selected;
    row.replaceChildren(...items.map(item => {
      const label = el('label','start-segment'); const input = el('input');
      input.type = 'radio'; input.name = name; input.value = item.id;
      input.checked = item.id === value; input.disabled = item.disabled === true;
      input.addEventListener('change',() => { if (input.checked) { value = item.id; changed(value); } });
      label.append(input,el('span','',item.label)); return label;
    }));
    if (focused) row.querySelectorAll('input').forEach(input => { if (input.checked) input.focus(); });
  };
  replace(choices,initial);
  /** @param {string} selected */
  const select = selected => {
    const inputs = [...row.querySelectorAll('input')];
    invariant(inputs.some(input => input.value === selected),'Missing choice');
    invariant(inputs.every(input => input.value !== selected || !input.disabled),'Choice is disabled');
    value = selected; inputs.forEach(input => { input.checked = input.value === selected; });
  };
  return {container,get value() { return value; },replace,select};
}

/** @param {string} title */
export function choiceSheet(title) {
  const dialog = el('dialog','start-sheet'); dialog.setAttribute('aria-label',title);
  const top = el('div','start-sheet-heading');
  top.append(el('h2','',title),button('Done',() => dialog.close(),'button quiet'));
  const body = el('div','start-sheet-body'); dialog.append(top,body);
  return {dialog,body,open:() => dialog.showModal(),destroy:() => { dialog.close(); dialog.remove(); }};
}

/** @param {import('../../types.js').Context} context */
export function modelPicker(context) {
  const container = el('div','start-model'); const sheet = choiceSheet('Choose a model');
  /** @type {Choice[]} */ let models = [];
  /** @type {Choice[]} */ let visible = [];
  let selected = ''; let disposed = false;
  const compact = segments('Model','start-model',[],'',choose);
  const expanded = segments('Configured models','start-model-all',[],'',choose);
  const more = button('All models',sheet.open,'button quiet start-more'); more.hidden = true;
  const status = el('p','muted small','Loading models…'); status.setAttribute('role','status');
  const retry = button('Retry models',() => { void load(); }); retry.hidden = true;
  sheet.body.append(expanded.container); container.append(compact.container,more,status,retry,sheet.dialog);
  /** @param {string} id */
  function choose(id) {
    invariant(models.some(model => model.id === id),'Model is not configured');
    selected = id;
    if (!visible.some(model => model.id === id)) {
      visible = models.slice(0,2);
      if (!visible.some(model => model.id === id)) visible[1] = models.find(model => model.id === id) ?? visible[0];
      invariant(visible.length > 0,'Missing model choices'); compact.replace(visible,id);
    } else compact.select(id);
    expanded.select(id);
  }
  async function load() {
    retry.hidden = true; status.textContent = 'Loading models…';
    try {
      const reply = await context.transport.request({cmd:'list_models'});
      invariant(reply.reply === 'models','Invalid model list reply');
      const configured = modelOptions(reply.data).filter(item => item.configured);
      if (disposed) return;
      if (!configured.length) throw new Error('Connect a model provider on your host to start an agent.');
      models = configured; const initial = (configured.find(item => item.default) ?? configured[0]).id;
      expanded.replace(models,initial); choose(initial);
      more.hidden = models.length <= 2; status.textContent = ''; status.hidden = true;
    } catch (cause) {
      if (!disposed) { status.hidden = false; status.textContent = errorText(cause); retry.hidden = false; }
    }
  }
  void load();
  return {container,get value() { return selected; },destroy:() => { disposed = true; sheet.destroy(); }};
}
