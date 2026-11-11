/** @param {HTMLElement} container @param {URLSearchParams} query */
export function applyIntent(container,query) {
  const action = query.get('action'); const value = query.get('value');
  if (!action) return;
  /** @type {Record<string,string>} */
  const labels = {rename:'Name',instructions:'Agent instructions',remember:'Memory',model:'Provider/model',effort:'Reasoning effort'};
  const label = [...container.querySelectorAll('label')].find(node => node.querySelector('.field-label')?.textContent === labels[action]);
  const input = label?.querySelector('input,textarea,select');
  if (value && input instanceof HTMLSelectElement && [...input.options].some(option => option.value === value)) {
    input.value = value;
    requestAnimationFrame(() => { input.focus(); input.scrollIntoView({block:'center'}); });
    return;
  }
  if (value && (input instanceof HTMLInputElement || input instanceof HTMLTextAreaElement)) {
    input.value = value; requestAnimationFrame(() => { input.focus(); input.scrollIntoView({block:'center'}); });
    return;
  }
  const name = action === 'stop' ? 'Interrupt' : action[0].toUpperCase()+action.slice(1);
  const button = [...container.querySelectorAll('button')].find(node => node.textContent === name);
  if (button) requestAnimationFrame(() => { button.focus(); button.scrollIntoView({block:'center'}); });
}
