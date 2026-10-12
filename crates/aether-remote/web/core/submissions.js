import { object, string } from './validate.js';
/** @typedef {import('../types.js').Reply} Reply */
/** @typedef {{outcome:'executed',reply:Reply,horizon:string}|{outcome:'absent_safe',horizon:string,retained_since:string}|{outcome:'unknown',reason:string,horizon:string,retained_since:string}} Resolution */
/** @param {Record<string,unknown>} welcome */
export function welcomeHorizon(welcome) {
  if (welcome.protocol_version !== 18 || !Object.hasOwn(welcome,'submission_horizon')) return null;
  return string(welcome.submission_horizon,'submission horizon');
}
/** @param {Reply} reply @returns {Resolution} */
export function submissionResolution(reply) {
  if (reply.reply !== 'submission_resolved') throw new Error('Expected submission_resolved receipt');
  const data = object(reply.data); const horizon = string(data.horizon,'submission horizon');
  if (data.outcome === 'executed') {
    const original = object(data.reply); string(original.reply,'original reply tag');
    return {outcome:'executed',horizon,reply:/** @type {Reply} */ (original)};
  }
  if (data.outcome !== 'absent_safe' && data.outcome !== 'unknown') throw new Error('Invalid submission outcome');
  const retained_since = string(data.retained_since,'retained since');
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(retained_since)) throw new Error('Invalid retained_since timestamp');
  if (data.outcome === 'absent_safe') return {outcome:'absent_safe',horizon,retained_since};
  if (!['no_horizon','foreign_horizon','horizon_ahead','forgotten'].includes(String(data.reason))) throw new Error('Invalid submission uncertainty reason');
  return {outcome:'unknown',reason:String(data.reason),horizon,retained_since};
}
