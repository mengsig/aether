/** @param {unknown} condition @param {string} message @returns {asserts condition} */
export function invariant(condition, message) {
  if (!condition) throw new Error(message);
}

/** @template {keyof HTMLElementTagNameMap} K @param {K} tag @param {string} [className] @param {string} [text] @returns {HTMLElementTagNameMap[K]} */
export function el(tag, className = '', text = '') {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

/** @param {string} label @param {()=>void} action @param {string} [className] */
export function button(label, action, className = 'button') {
  const node = el('button', className, label);
  node.type = 'button';
  node.addEventListener('click', action);
  return node;
}

/** @param {string} title @param {string} [subtitle] */
export function heading(title, subtitle = '') {
  const header = el('header', 'screen-heading');
  header.append(el('h1', '', title));
  if (subtitle) header.append(el('p', 'muted', subtitle));
  return header;
}

/** @param {string} title @param {string} detail */
export function empty(title, detail) {
  const node = el('div', 'empty-state');
  node.append(el('h2', '', title), el('p', 'muted', detail));
  return node;
}

/** @param {unknown} error */
export function errorText(error) {
  return error instanceof Error ? error.message : String(error);
}

/** @param {string} label @param {HTMLElement} control */
export function field(label, control) {
  const wrapper = el('label', 'field');
  wrapper.append(el('span', 'field-label', label), control);
  return wrapper;
}

/** @param {number|null|undefined} value */
export function number(value) {
  return value == null ? '—' : Intl.NumberFormat(undefined, {notation:'compact', maximumFractionDigits:1}).format(value);
}

/** @param {number|null|undefined} value */
export function money(value) {
  return value == null ? 'Not reported' : Intl.NumberFormat(undefined, {style:'currency',currency:'USD',maximumFractionDigits:2}).format(value);
}
