import { record } from './protocol.js';

/** @param {unknown} value */
export function words(value) {
  if (typeof value !== 'string' || !value.trim()) return 'Not reported';
  const text = value.replaceAll('_', ' ');
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** @param {unknown} value */
export function count(value) {
  if (value == null) return 'Not reported';
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) return 'Invalid reported count';
  return new Intl.NumberFormat().format(value);
}

/** @param {unknown} value */
export function dollars(value) {
  if (value == null) return 'Not reported';
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) return 'Invalid reported cost';
  if (value > 0 && value < 0.01) return '<$0.01';
  return new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' }).format(value);
}

/** @param {unknown} value @param {boolean} [dateOnly] */
export function dateLabel(value, dateOnly = false) {
  if (value == null) return 'Not reported';
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}T/.test(value)) return 'Invalid reported date';
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return 'Invalid reported date';
  return new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', year: 'numeric',
    ...(!dateOnly ? { hour: /** @type {const} */ ('numeric'), minute: /** @type {const} */ ('2-digit'), timeZoneName: /** @type {const} */ ('short') } : {}) }).format(date);
}

/** @param {unknown} value */
export function fileSize(value) {
  if (value == null) return 'Size not reported';
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) return 'Invalid reported size';
  if (value < 1024) return `${count(value)} bytes`;
  const unit = value < 1024 ** 2 ? 1024 : 1024 ** 2;
  return `${new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(value / unit)} ${unit === 1024 ? 'KiB' : 'MiB'}`;
}

/** @param {unknown} value */
export function recurrenceLabel(value) {
  if (value == null) return 'Recurrence not reported';
  const recurrence = record(value);
  if (recurrence.kind === 'interval') {
    const seconds = recurrence.seconds;
    if (typeof seconds !== 'number' || !Number.isSafeInteger(seconds) || seconds <= 0) return 'Invalid reported interval';
    if (seconds % 86400 === 0) return `Every ${count(seconds / 86400)} day${seconds === 86400 ? '' : 's'}`;
    if (seconds % 3600 === 0) return `Every ${count(seconds / 3600)} hour${seconds === 3600 ? '' : 's'}`;
    if (seconds % 60 === 0) return `Every ${count(seconds / 60)} minute${seconds === 60 ? '' : 's'}`;
    return `Every ${count(seconds)} second${seconds === 1 ? '' : 's'}`;
  }
  if (recurrence.kind !== 'cron') return `Recurrence: ${words(recurrence.kind)}`;
  const expression = typeof recurrence.expression === 'string' ? recurrence.expression.trim() : '';
  const daily = /^(\d{1,2}) (\d{1,2}) \* \* \*$/.exec(expression);
  const zone = typeof recurrence.timezone === 'string' && recurrence.timezone ? recurrence.timezone : 'timezone not reported';
  if (daily && Number(daily[1]) < 60 && Number(daily[2]) < 24) return `Daily at ${daily[2].padStart(2, '0')}:${daily[1].padStart(2, '0')} · ${zone}`;
  return `Custom schedule · ${zone}`;
}

/** @param {unknown} value @param {import('../../types.js').Agent[]} agents */
export function scopeLabel(value, agents) {
  if (value == null) return 'Scope not reported';
  const scope = record(value);
  if (scope.scope === 'global' || scope.scope === 'everywhere') return 'Everywhere';
  if (scope.scope === 'repository') return typeof scope.root === 'string' ? scope.root : 'Repository';
  const agent = agents.find(agent => agent.id === scope.id);
  if (scope.scope === 'agent') return agent?.name || 'An agent';
  if (scope.scope === 'subtree') return agent ? `${agent.name} and descendants` : 'An agent and descendants';
  if (scope.scope === 'run') return 'One run';
  return words(scope.scope);
}

/** @param {Record<string,unknown>} usage */
export function costCoverage(usage) {
  const coverage = usage.cost_complete === false ? 'Partial coverage · total cost may be higher'
    : usage.cost_complete === true ? 'All reported usage priced' : 'Pricing coverage unknown';
  const sources = { list_price: 'List-price estimate', provider_reported: 'Provider-reported cost', mixed: 'Mixed cost sources', unknown: 'Cost source unknown' };
  const source = sources[/** @type {keyof typeof sources} */ (usage.cost_basis)] || 'Cost source unknown';
  return `${coverage}. ${source}.`;
}
