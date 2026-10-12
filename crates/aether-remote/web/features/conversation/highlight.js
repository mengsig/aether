import {el, invariant} from '../../ui/dom.js';

/** @param {string} source @param {string} language */
export function highlight(source, language) {
  invariant(typeof source === 'string', 'Code must be text');
  invariant(typeof language === 'string', 'Code language must be text');
  const code = el('code');
  if (source.length > 200_000) { code.textContent = source; return code; }
  if (/^(diff|patch)$/i.test(language)) {
    for (const line of source.split('\n')) {
      const kind = line.startsWith('+') ? 'added' : line.startsWith('-') ? 'removed' : 'context';
      code.append(el('span', `code-line code-${kind}`, `${line}\n`));
    }
    return code;
  }
  const pattern = /("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|\/\/[^\n]*|#[^\n]*|\b(?:const|let|var|function|return|if|else|class|import|export|from|async|await|def|for|while|fn|pub|use|impl|match|true|false|null|None)\b|\b\d+(?:\.\d+)?\b)/g;
  let start = 0;
  for (const match of source.matchAll(pattern)) {
    code.append(document.createTextNode(source.slice(start, match.index)));
    const token = match[0];
    const kind = /^["'`]/.test(token) ? 'string' : /^(\/\/|#)/.test(token) ? 'comment' : /^\d/.test(token) ? 'number' : 'keyword';
    code.append(el('span', `code-${kind}`, token));
    start = (match.index ?? 0) + token.length;
  }
  code.append(document.createTextNode(source.slice(start)));
  return code;
}
