const AGENT_ID = /^agt_[0-9A-HJKMNP-TV-Z]{26}$/;
const APPROVAL_ID = /^apr_[0-9A-HJKMNP-TV-Z]{26}$/;
const KINDS = new Set(['approval','question','agent_failed','agent_blocked','task_completed','task_failed','test']);

/** @param {unknown} value @param {string} daemonId */
export function notificationData(value, daemonId) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid notification payload');
  const data = /** @type {Record<string,unknown>} */ (value);
  if (data.v !== 1 || data.instance_id !== daemonId) throw new Error('Notification belongs to a different daemon');
  if (typeof data.title !== 'string' || !data.title || !Number.isSafeInteger(data.seq) || Number(data.seq) < 0) throw new Error('Invalid notification metadata');
  if (typeof data.at !== 'string' || !Number.isFinite(Date.parse(data.at))) throw new Error('Invalid notification time');
  if (typeof data.url !== 'string' || data.url !== safeNotificationRoute(data.url) || data.url !== expectedRoute(data)) throw new Error('Invalid notification deep link');
  const agentName = clipped(data.agent_name,80); const preview = clipped(data.preview,200);
  const attention = data.kind === 'approval' || data.kind === 'question';
  const category = attention ? 'attention' : data.kind === 'test' ? 'test' : 'status';
  return {title:clipped(data.title,160),body:[agentName,preview || 'Open Aether to review this update.'].filter(Boolean).join(' · '),
    url:data.url,tag:`aether-${daemonId}-${String(data.agent ?? 'fleet')}-${category}`};
}

/** @param {Record<string,unknown>} data */
function expectedRoute(data) {
  if (!KINDS.has(String(data.kind))) throw new Error('Unknown notification kind');
  if (data.agent != null && (typeof data.agent !== 'string' || data.agent.length !== 30 || !AGENT_ID.test(data.agent))) throw new Error('Invalid notification agent');
  if (data.kind === 'test' || !data.agent) return '#activity';
  if (data.kind === 'approval') {
    if (typeof data.approval !== 'string' || data.approval.length !== 30 || !APPROVAL_ID.test(data.approval)) throw new Error('Invalid notification approval');
    return `#attention?item=${data.approval}`;
  }
  if (data.kind === 'question') {
    if (!Number.isSafeInteger(data.question_seq) || Number(data.question_seq) < 1) throw new Error('Invalid notification question');
    return `#attention?item=question-${String(data.question_seq)}`;
  }
  return `#agent/${String(data.agent)}`;
}

/** @param {unknown} value @param {number} limit */
function clipped(value,limit) {
  if (value == null) return '';
  if (typeof value !== 'string') throw new Error('Invalid notification text');
  return Array.from(value).slice(0,limit).join('').replace(/\s+/g,' ');
}

/** @param {unknown} value */
export function safeNotificationRoute(value) {
  if (value === '#activity') return value;
  if (typeof value !== 'string' || value.trim() !== value) return '#activity';
  if (value.startsWith('#agent/') && AGENT_ID.test(value.slice(7))) return value;
  if (value.startsWith('#attention?item=') && APPROVAL_ID.test(value.slice(16))) return value;
  const question = /^#attention\?item=question-([1-9][0-9]*)$/.exec(value);
  if (question && Number.isSafeInteger(Number(question[1]))) return value;
  return '#activity';
}
