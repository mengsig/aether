import {button, el, invariant} from '../../ui/dom.js';
import {markdown} from './markdown.js';
import {blockText, entryText, inboxPresentation, startsTimeGroup} from './state.js';
import {messageActions} from './actions.js';

/** @param {import('../../types.js').TranscriptEntry} entry @param {import('../../types.js').Context} context @param {(text:string)=>void} quote @param {string} agentId @param {import('../../types.js').TranscriptEntry} [previous] */
export function messageCard(entry, context, quote, agentId, previous) {
  invariant(Number.isSafeInteger(entry.seq), 'Message sequence must be an integer');
  invariant(Array.isArray(entry.message.content), 'Message content must be an array');
  const toolsOnly = entry.message.content.every(block => ['tool_use', 'tool_result', 'thinking'].includes(String(block.type)));
  const role = toolsOnly ? 'tool' : entry.message.role === 'user' ? 'user' : 'assistant';
  const node = el('article', `conversation-message conversation-role-${role}`);
  node.dataset.seq = String(entry.seq);
  node.setAttribute('aria-label', `${entry.message.role} message`);
  const time = el('time', 'muted', displayTime(entry.at));
  time.dateTime = entry.at;
  if (startsTimeGroup(entry, previous)) {
    const group = el('time', 'conversation-time-group muted', new Date(entry.at).toLocaleString([], {month:'short', day:'numeric', hour:'2-digit', minute:'2-digit'}));
    group.dateTime = entry.at;
    node.append(group);
  }
  const content = el('div', 'conversation-content');
  const originals = [];
  for (const block of entry.message.content) {
    const envelopes = block.type === 'text' ? inboxPresentation(entry.message.role, String(block.text ?? ''), context.store.get().agents) : null;
    if (!envelopes) { content.append(contentBlock(block, context, agentId)); continue; }
    for (const envelope of envelopes) {
      content.append(el('p', 'conversation-source muted', envelope.caption), contentBlock({...block, text:envelope.body}, context, agentId));
    }
    originals.push(String(block.text));
  }
  const provenance = el('details', 'conversation-provenance');
  provenance.hidden = true;
  const summary = el('summary', 'muted', 'Message details · ');
  summary.setAttribute('aria-label', `Message provenance · ${entry.message.role} · ${displayTime(entry.at)}`);
  summary.append(time);
  provenance.append(summary, el('p', 'muted', `Sequence ${entry.seq} · ${entry.at}${entry.invocation ? ` · Invocation ${entry.invocation}` : ''}`));
  for (const original of originals) provenance.append(el('p', '', 'Original message'), el('pre', '', original));
  provenance.addEventListener('toggle', () => { if (!provenance.open) provenance.hidden = true; });
  node.append(content, provenance);
  messageActions(node, entryText(entry), context, quote);
  return node;
}

/** @param {string} value */
function displayTime(value) {
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toLocaleTimeString([], {hour:'2-digit', minute:'2-digit'}) : value;
}

/** @param {Record<string,unknown>} block @param {import('../../types.js').Context} context @param {string} agentId */
function contentBlock(block, context, agentId) {
  if (block.type === 'text') return markdown(String(block.text ?? ''), context.toast);
  if (block.type === 'image') return imageBlock(block);
  const details = el('details', 'conversation-tool');
  const failed = block.is_error === true || block.type === 'error';
  details.classList.toggle('conversation-error', failed);
  details.open = failed;
  const title = block.type === 'thinking' ? 'Reasoning' : block.type === 'tool_use' ? String(block.name ?? 'Tool call') : block.type === 'tool_result' ? `Tool ${failed ? 'failed' : 'result'}` : String(block.type ?? 'Event');
  details.append(el('summary', '', title));
  if (block.type === 'tool_use') {
    details.append(el('pre', 'conversation-tool-input', JSON.stringify(block.input, null, 2) ?? 'No input'));
    details.append(el('p', 'muted', `Call ${String(block.id ?? 'not reported')}`));
  } else if (block.type === 'thinking' && block.redacted) {
    details.append(el('p', 'muted', 'Reasoning was redacted by the provider.'));
  } else {
    details.append(markdown(blockText(block), context.toast));
    if (block.tool_use_id) details.append(el('p', 'muted', `Call ${String(block.tool_use_id)}`));
  }
  appendReferences(details, block, context, agentId);
  return details;
}

/** @param {Record<string,unknown>} block */
function imageBlock(block) {
  const wrapper = el('div', 'conversation-image');
  const allowed = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'];
  if (typeof block.data !== 'string' || block.data.length > 12_000_000 || !allowed.includes(String(block.media_type)) || !/^[A-Za-z0-9+/=\s]+$/.test(block.data)) {
    wrapper.append(el('p', 'muted', 'Image unavailable: unsupported format or size.'));
    return wrapper;
  }
  const image = el('img');
  image.alt = 'Image included in the conversation';
  image.loading = 'lazy';
  image.src = `data:${block.media_type};base64,${block.data}`;
  image.addEventListener('error', () => wrapper.replaceChildren(el('p', 'muted', 'Image could not be decoded.')), {once:true});
  wrapper.append(image);
  return wrapper;
}

/** @param {HTMLElement} node @param {Record<string,unknown>} block @param {import('../../types.js').Context} context @param {string} agentId */
function appendReferences(node, block, context, agentId) {
  const text = JSON.stringify(block);
  const agents = new Set(text.match(/\bagt_[A-Za-z0-9]+\b/g) ?? []);
  const artifacts = new Set(text.match(/\bart_[A-Za-z0-9]+\b/g) ?? []);
  const links = el('div', 'row conversation-references');
  for (const id of [...agents].slice(0, 12)) {
    const agent = context.store.get().agents.find(item => item.id === id);
    const link = el('a', 'button', `Open ${agent?.name || id}`);
    link.href = `#agent/${encodeURIComponent(id)}`;
    link.addEventListener('click', event => { if (!event.ctrlKey && !event.metaKey) { event.preventDefault(); context.navigate(`agent/${id}`); } });
    links.append(link);
  }
  for (const id of [...artifacts].slice(0, 12)) {
    links.append(button(`Artifact ${id}`, () => context.navigate(`controls/${agentId}?panel=artifacts&artifact=${id}`)));
  }
  if (links.childElementCount) node.append(links);
}
