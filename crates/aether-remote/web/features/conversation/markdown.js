import {button, el, errorText, invariant} from '../../ui/dom.js';
import {highlight} from './highlight.js';

/** @typedef {{kind:string,text:string,language?:string,level?:number}} Block */
/** @typedef {{kind:string,text:string,href?:string}} Inline */

/** @param {string} href @returns {string|null} */
export function safeHref(href) {
  if (!href || /[\u0000-\u0020\u007f]/.test(href)) return null;
  if (/^(https?:\/\/|mailto:)/i.test(href)) return href;
  return null;
}

/** @param {string} text @returns {Inline[]} */
export function inlineTokens(text) {
  invariant(typeof text === 'string', 'Markdown inline input must be text');
  const pattern = /(`[^`\n]+`|\*\*[^*\n]+\*\*|~~[^~\n]+~~|\*[^*\n]+\*|\[[^\]\n]+\]\([^\s)]+\))/g;
  /** @type {Inline[]} */
  const tokens = [];
  let start = 0;
  for (const match of text.matchAll(pattern)) {
    if (match.index > start) tokens.push({kind:'text', text:text.slice(start, match.index)});
    tokens.push(parseInline(match[0]));
    start = match.index + match[0].length;
  }
  if (start < text.length) tokens.push({kind:'text', text:text.slice(start)});
  invariant(start <= text.length, 'Inline cursor exceeded source');
  return tokens;
}

/** @param {string} token @returns {Inline} */
function parseInline(token) {
  const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(token);
  if (link) return {kind:'link', text:link[1], href:safeHref(link[2]) ?? undefined};
  if (token.startsWith('`')) return {kind:'code', text:token.slice(1,-1)};
  if (token.startsWith('**')) return {kind:'strong', text:token.slice(2,-2)};
  if (token.startsWith('~~')) return {kind:'del', text:token.slice(2,-2)};
  return {kind:'em', text:token.slice(1,-1)};
}

/** @param {string} text @returns {Block[]} */
export function parseMarkdown(text) {
  invariant(typeof text === 'string', 'Markdown input must be text');
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  /** @type {Block[]} */
  const blocks = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index++];
    if (!line.trim()) continue;
    const fence = /^\s{0,3}(`{3,}|~{3,})([^ ]*)/.exec(line);
    if (fence) {
      const content = [];
      const closing = new RegExp(`^\\s{0,3}${fence[1][0]}{${fence[1].length},}\\s*$`);
      while (index < lines.length && !closing.test(lines[index])) content.push(lines[index++]);
      if (index < lines.length) index++;
      blocks.push({kind:'code', language:fence[2], text:content.join('\n')});
      continue;
    }
    const heading = /^(#{1,6})\s+(.+)$/.exec(line);
    if (heading) { blocks.push({kind:'heading', level:heading[1].length, text:heading[2]}); continue; }
    if (/^\s*(?:---+|\*\*\*+)\s*$/.test(line)) { blocks.push({kind:'rule', text:''}); continue; }
    const quote = /^>\s?(.*)$/.exec(line);
    if (quote) { blocks.push({kind:'quote', text:quote[1]}); continue; }
    const list = /^\s*(?:([-+*])|\d+[.)])\s+(.+)$/.exec(line);
    if (list) { blocks.push({kind:list[1] ? 'ul' : 'ol', text:list[2]}); continue; }
    const previous = blocks.at(-1);
    if (previous?.kind === 'paragraph' && index > 1 && lines[index-2].trim()) previous.text += `\n${line}`;
    else blocks.push({kind:'paragraph', text:line});
  }
  invariant(index === lines.length, 'Markdown parser did not consume source');
  return blocks;
}

/** @param {HTMLElement} target @param {string} text */
export function appendInline(target, text) {
  for (const token of inlineTokens(text)) {
    if (token.kind === 'text' || (token.kind === 'link' && !token.href)) {
      target.append(document.createTextNode(token.text));
      continue;
    }
    if (token.kind === 'link') {
      const link = el('a', '', token.text);
      link.href = token.href ?? '';
      link.target = '_blank';
      link.rel = 'noopener noreferrer';
      target.append(link);
      continue;
    }
    const tag = token.kind === 'code' ? 'code' : token.kind === 'strong' ? 'strong' : token.kind === 'del' ? 'del' : 'em';
    target.append(el(tag, '', token.text));
  }
}

/** @param {string} text @param {(message:string)=>void} report */
export async function copyText(text, report) {
  try {
    if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable. Select the text and copy from your browser.');
    await navigator.clipboard.writeText(text);
    report('Copied');
  } catch (error) { report(errorText(error)); }
}

/** @param {string} text @param {(message:string)=>void} report */
export function markdown(text, report) {
  const root = el('div', 'conversation-markdown');
  for (const block of parseMarkdown(text)) {
    if (block.kind === 'code') {
      const wrapper = el('div', 'conversation-code');
      const toolbar = el('div', 'row');
      toolbar.append(el('span', 'muted', block.language || 'Code'), button('Copy code', () => { void copyText(block.text, report); }));
      const pre = el('pre');
      pre.append(highlight(block.text, block.language ?? ''));
      wrapper.append(toolbar, pre);
      root.append(wrapper);
      continue;
    }
    if (block.kind === 'rule') { root.append(el('hr')); continue; }
    if (block.kind === 'ul' || block.kind === 'ol') {
      const previous = root.lastElementChild;
      const list = previous?.tagName.toLowerCase() === block.kind ? previous : el(block.kind);
      const item = el('li');
      appendInline(item, block.text);
      list.append(item);
      if (!list.parentNode) root.append(list);
      continue;
    }
    const tag = block.kind === 'heading' ? 'h3' : block.kind === 'quote' ? 'blockquote' : 'p';
    const node = el(tag);
    if (block.kind === 'heading') node.setAttribute('aria-level', String(block.level));
    appendInline(node, block.text);
    root.append(node);
  }
  return root;
}
