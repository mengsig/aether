import {invariant} from '../../ui/dom.js';

/** @typedef {import('../../types.js').TranscriptEntry} Entry */
/** @param {Record<string,unknown>} block */
export function blockText(block) {
  if (typeof block.text === 'string') return block.text;
  if (block.type === 'image') return '[Image]';
  if (block.type === 'tool_use') return `${String(block.name ?? 'Tool')}\n${JSON.stringify(block.input, null, 2) ?? ''}`;
  if (Array.isArray(block.content)) return block.content.map(item => typeof item?.text === 'string' ? item.text : '[Non-text output]').join('\n');
  return JSON.stringify(block, null, 2);
}

/** @param {Entry} entry */
export function entryText(entry) {
  invariant(Array.isArray(entry.message.content), 'Transcript message requires content blocks');
  invariant(typeof entry.message.role === 'string', 'Transcript message requires a role');
  return entry.message.content.map(blockText).join('\n\n');
}

/** @param {string} role @param {string} text @param {import('../../types.js').Agent[]} [agents] */
export function operatorEnvelope(role, text, agents = []) {
  invariant(typeof role === 'string', 'Envelope presentation requires a transcript role');
  invariant(typeof text === 'string', 'Envelope presentation requires text');
  if (role !== 'user') return null;
  const match = /^\[inbox [1-9]\d*\] Message from ([^\r\n]+?):( \[authenticated operator instruction\])?\r?\n([\s\S]+)$/.exec(text);
  if (!match) return triggerEnvelope(text);
  const caption = messageSource(match[1], Boolean(match[2]), agents);
  return caption && match[3].trim() ? {body:match[3], original:text, caption} : null;
}

const ULID = '[0-9A-HJKMNP-TV-Z]{26}';
const USER = "user(?::[A-Za-z0-9 _'-]+)?";

/** @param {string} role @param {string} text @param {import('../../types.js').Agent[]} agents */
export function inboxPresentation(role, text, agents) {
  invariant(typeof text === 'string', 'Inbox presentation requires original text');
  invariant(Array.isArray(agents), 'Inbox presentation requires known agents');
  const pieces = text.split(/(?=^\[inbox [1-9]\d*\] (?:Message from |External trigger from ))/m);
  const envelopes = pieces.map(piece => operatorEnvelope(role, piece, agents));
  if (envelopes.some(envelope => !envelope)) return null;
  return envelopes.filter(envelope => envelope !== null);
}

/** Presentation hints never grant authority; transcript text has no typed sender. */
/** @param {string} source @param {boolean} operator @param {import('../../types.js').Agent[]} agents */
function messageSource(source, operator, agents) {
  invariant(typeof source === 'string' && source.length > 0, 'Message source requires a label');
  invariant(Array.isArray(agents), 'Agent names must come from the store');
  if (operator) {
    if (new RegExp(`^${USER}:local_socket#${ULID}$`).test(source)) return 'From this computer';
    if (new RegExp(`^${USER}:tcp:(?:[0-9.]+|\\[[0-9a-fA-F:]+\\]):[0-9]{1,5}#${ULID}$`).test(source)) return 'From a network connection';
    if (new RegExp(`^${USER}:device:(?:[A-Za-z0-9 _'-]|%[0-9A-F]{2})+\\(dev_${ULID}\\)#${ULID}$`).test(source)) return 'From a paired device';
    return new RegExp(`^${USER}$`).test(source) ? 'From an operator' : null;
  }
  if (new RegExp(`^agt_${ULID}$`).test(source)) return `From ${agents.find(agent => agent.id === source)?.name || 'an agent'}`;
  if (source === 'scheduler') return 'From the scheduler';
  if (source === 'system') return 'From the system';
  if (new RegExp(`^call_${ULID}$`).test(source)) return 'From a tool';
  if (new RegExp(`^rule:rule_${ULID}$`).test(source)) return 'From an approval rule';
  if (/^client:[A-Za-z0-9_-]+$/.test(source)) return 'From a client';
  return null;
}

/** @param {string} text */
function triggerEnvelope(text) {
  const match = /^\[inbox [1-9]\d*\] External trigger from ([a-zA-Z0-9_.:-]+)( \[authenticated operator instruction\])?: (.+)\s*$/.exec(text);
  if (!match || (match[2] && match[1] !== 'github')) return null;
  if (match[1].startsWith('schedule:') && !new RegExp(`^schedule:sch_${ULID}$`).test(match[1])) return null;
  try { JSON.parse(match[3]); }
  catch (error) { if (error instanceof SyntaxError) return null; throw error; }
  const caption = match[1].startsWith('schedule:') ? 'Scheduled message' : 'External trigger';
  return {body:match[3], original:text, caption};
}

/** @param {Entry} entry @param {Entry|undefined} previous */
export function startsTimeGroup(entry, previous) {
  const at = Date.parse(entry.at);
  invariant(Number.isFinite(at), 'Time group requires a valid timestamp');
  if (!previous) return true;
  const before = Date.parse(previous.at);
  invariant(Number.isFinite(before), 'Previous time group requires a valid timestamp');
  return Math.abs(at - before) >= 300_000 || new Date(at).toDateString() !== new Date(before).toDateString();
}

/** @param {Entry[]} entries @param {string} query */
export function searchEntries(entries, query) {
  invariant(Array.isArray(entries), 'Search requires transcript entries');
  invariant(typeof query === 'string', 'Search query must be text');
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return [];
  return entries.flatMap((entry, index) => entryText(entry).toLocaleLowerCase().includes(needle) ? [index] : []);
}

/** @param {string} draft @param {number} cursor */
export function mentionAt(draft, cursor) {
  invariant(typeof draft === 'string', 'Draft must be text');
  invariant(Number.isInteger(cursor) && cursor >= 0 && cursor <= draft.length, 'Draft cursor out of range');
  const match = /(?:^|\s)@([^\s@]*)$/.exec(draft.slice(0, cursor));
  return match ? {start:cursor - match[1].length - 1, end:cursor, query:match[1]} : null;
}

/** @param {string} draft @param {{start:number,end:number}} mention @param {string} path */
export function replaceMention(draft, mention, path) {
  invariant(mention.start >= 0 && mention.end <= draft.length && mention.start < mention.end, 'Mention range out of bounds');
  invariant(path.length > 0 && !/[\r\n\u0000]/.test(path), 'Invalid mention path');
  const reference = /\s/.test(path) ? `@${JSON.stringify(path)} ` : `@${path} `;
  return {text:draft.slice(0, mention.start) + reference + draft.slice(mention.end), cursor:mention.start + reference.length};
}

/** @param {import('../../types.js').Outbox['status']} status */
export function deliveryLabel(status) {
  const labels = {pending:'Sending…', sent:'Delivered to inbox', failed:'Not delivered', unknown:'Delivery unconfirmed', dismissed:'Reviewed'};
  invariant(Object.hasOwn(labels, status), 'Unknown delivery status');
  return labels[status];
}
