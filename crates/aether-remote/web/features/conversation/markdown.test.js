import test from 'node:test';
import assert from 'node:assert/strict';
import {inlineTokens, parseMarkdown, safeHref} from './markdown.js';

test('markdown preserves HTML as literal text, never executable markup', () => {
  const source = '<img src=x onerror=alert(1)>\n<script>bad()</script>';
  assert.deepEqual(parseMarkdown(source), [{kind:'paragraph', text:source}]);
  assert.deepEqual(inlineTokens(source), [{kind:'text', text:source}]);
});

test('only explicit web and mail destinations are linkable', () => {
  for (const url of ['javascript:alert(1)', 'data:text/html,bad', 'file:///etc/passwd', '//evil.test', 'java\nscript:bad', 'https://ok.test\u0000bad', ' https://example.org']) assert.equal(safeHref(url), null);
  assert.equal(safeHref('https://example.org/path?q=value'), 'https://example.org/path?q=value');
  assert.equal(safeHref('mailto:user@example.org'), 'mailto:user@example.org');
  assert.equal(inlineTokens('[open](javascript:bad)')[0].href, undefined);
});

test('fences retain source and ignore markdown inside code', () => {
  const blocks = parseMarkdown('# Changes\n\n```diff\n-old\n+new\n<script>unsafe()</script>\n```\n\nDone');
  assert.equal(blocks[0].kind, 'heading');
  assert.deepEqual(blocks[1], {kind:'code',language:'diff',text:'-old\n+new\n<script>unsafe()</script>'});
  assert.equal(blocks[2].text, 'Done');
});

test('unfinished streaming fences remain code; longer fences can contain shorter ones', () => {
  assert.deepEqual(parseMarkdown('```js\nconst x = 1;'), [{kind:'code',language:'js',text:'const x = 1;'}]);
  assert.equal(parseMarkdown('````md\n```js\ncode\n```\n````')[0].text, '```js\ncode\n```');
});

test('blocks and inline formatting retain readable content', () => {
  const blocks = parseMarkdown('## Header\r\n\r\n- One\r\n- Two\r\n\r\n> Quote\r\n\r\n1. First\r\n\r\n---');
  assert.deepEqual(blocks.map(block => block.kind), ['heading','ul','ul','quote','ol','rule']);
  const tokens = inlineTokens('A **bold** and `literal` with *emphasis* and ~~old~~.');
  assert.deepEqual(tokens.filter(token => token.kind !== 'text').map(token => token.kind), ['strong','code','em','del']);
  assert.equal(tokens.map(token => token.text).join(''), 'A bold and literal with emphasis and old.');
});
