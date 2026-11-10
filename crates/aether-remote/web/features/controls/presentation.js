import { el, invariant } from '../../ui/dom.js';
import { record, required } from './protocol.js';
import { details } from './ui.js';
import { count, dollars, costCoverage, dateLabel, words } from './format.js';

/** @param {Record<string,unknown>} data @param {string} [label] */
export function rawDetails(data, label = 'Inspect original record') {
  record(data);
  required(label, 'Disclosure label');
  const root = el('details', 'controls-record');
  const content = el('pre', 'controls-preview', JSON.stringify(data, null, 2));
  content.tabIndex = 0;
  content.setAttribute('aria-label', label);
  root.append(el('summary', '', label), content);
  return root;
}

/** @param {unknown} value */
export function usageSummary(value) {
  const root = el('div', 'stack controls-usage');
  if (value == null) { root.append(el('p', 'muted', 'Usage not reported. Cost and token counts are unknown.')); return root; }
  const usage = record(value);
  const metrics = el('dl', 'controls-metrics');
  for (const [label, value] of Object.entries({ 'Reported cost · USD': dollars(usage.cost_usd), 'Input tokens': count(usage.input_tokens), 'Output tokens': count(usage.output_tokens) })) {
    const metric = el('div');
    metric.append(el('dt', '', label), el('dd', '', value));
    metrics.append(metric);
  }
  root.append(metrics, el('p', 'muted controls-caption', costCoverage(usage)));
  const more = el('details', 'controls-record');
  more.append(el('summary', '', 'More usage details'), details({ 'Cache read tokens': count(usage.cache_read_tokens),
    'Cache write tokens': count(usage.cache_write_tokens), 'Reasoning tokens': count(usage.reasoning_tokens),
    'Unbilled estimate · USD': dollars(usage.estimated_cost_usd) }));
  root.append(more);
  return root;
}

/** @param {Record<string,unknown>} run @param {import('../../types.js').Agent[]} agents */
export function runTitle(run, agents) {
  required(run.run, 'Run ID');
  invariant(Array.isArray(agents), 'Run titles require an agent list.');
  if (typeof run.objective === 'string' && run.objective.trim()) return run.objective;
  const root = agents.find(agent => agent.run === run.run && !agent.parent);
  if (root) return root.name;
  return run.started_at == null ? 'Stored run' : `Run · ${dateLabel(run.started_at, true)}`;
}

/** @param {Record<string,unknown>} result */
export function runReport(result) {
  const body = record(result.body);
  const root = el('section', 'stack controls-report');
  root.append(el('h3', '', typeof body.objective === 'string' ? body.objective : 'Run report'));
  if (typeof body.summary === 'string') root.append(el('p', 'controls-prose', body.summary));
  root.append(details({ Outcome: words(body.outcome), Generated: dateLabel(result.generated_at) }), usageSummary(body.usage), rawDetails(result, 'Inspect full report'));
  return root;
}
