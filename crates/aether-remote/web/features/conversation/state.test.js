import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {blockText, deliveryLabel, entryText, mentionAt, operatorEnvelope, replaceMention, searchEntries} from './state.js';
import {commandRoute, commands, findCommands, parseCommand} from './commands.js';

/** @param {number} seq @param {string} text */
const entry = (seq, text) => ({seq, at:'2026-10-12T00:00:00Z', message:{role:'assistant',content:[{type:'text',text}]}});

test('recognized operator envelopes retain exact original text without assigning identity', () => {
  const original = '[inbox 1] Message from user:device:Live operator phone(dev_01M4PKDT0T1FFRP9C7Q6N2T75V)#01M4PKMS9YTNDAYSSZ1Z0YEPG0: [authenticated operator instruction]\nCount slowly';
  assert.deepEqual(operatorEnvelope('user', original), {body:'Count slowly', original, caption:'From a paired device'});
  assert.equal(operatorEnvelope('assistant', original), null);
  assert.equal(operatorEnvelope('user', `Quoted message\n${original}`), null);
  assert.equal(operatorEnvelope('user', original.replace('dev_01M4PKDT0T1FFRP9C7Q6N2T75V', 'dev_unknown')), null);
  assert.equal(operatorEnvelope('user', original.replace('[authenticated operator instruction]', '[unverified]')), null);
  assert.equal(operatorEnvelope('user', original.replace('\nCount slowly', '')), null);
  assert.deepEqual(searchEntries([{...entry(1, original), message:{role:'user', content:[{type:'text',text:original}]}}], 'dev_01M4PKDT'), [0]);
});

test('search includes tool inputs and outputs with literal case-insensitive matching', () => {
  const entries = [entry(1, 'One'), entry(2, 'Mixed CASE')];
  entries.push({seq:3, at:'', message:{role:'assistant',content:[{type:'text', text:blockText({type:'tool_use',name:'exec',input:{cmd:'npm test'}})}]}});
  assert.deepEqual(searchEntries(entries, 'case'), [1]);
  assert.deepEqual(searchEntries(entries, 'npm test'), [2]);
  assert.deepEqual(searchEntries(entries, ''), []);
  assert.deepEqual(searchEntries(entries, '[a-z]+'), []);
  assert.match(entryText(entries[2]), /npm test/);
});

test('file mentions use the cursor and preserve surrounding multiline draft', () => {
  const draft = 'First line\nReview @src/com after';
  const cursor = draft.indexOf(' after');
  const mention = mentionAt(draft, cursor);
  assert.deepEqual(mention, {start:18,end:26,query:'src/com'});
  assert.ok(mention);
  const result = replaceMention(draft, mention, 'src/component.js');
  assert.equal(result.text, 'First line\nReview @src/component.js  after');
  assert.equal(result.text.slice(0, result.cursor), 'First line\nReview @src/component.js ');
  assert.equal(mentionAt('mail@example.org', 16), null);
});

test('mention paths with whitespace are quoted and invalid boundaries fail visibly', () => {
  assert.equal(replaceMention('@a', {start:0,end:2}, 'a b.js').text, '@"a b.js" ');
  assert.throws(() => mentionAt('short', 10), /cursor/);
  assert.throws(() => replaceMention('@a', {start:0,end:3}, 'x'), /range/);
  assert.throws(() => replaceMention('@a', {start:0,end:2}, 'bad\nfile'), /path/);
});

test('delivery labels distinguish acknowledgement from model response and uncertainty', () => {
  assert.equal(deliveryLabel('sent'), 'Delivered to inbox');
  assert.equal(deliveryLabel('unknown'), 'Delivery unconfirmed');
  assert.notEqual(deliveryLabel('failed'), deliveryLabel('pending'));
});

test('slash palette covers every public TUI operation', () => {
  const registry = readFileSync(new URL('../../../../aether-tui/src/commands/registry.rs', import.meta.url), 'utf8');
  const names = [...registry.matchAll(/\bname: "([a-z]+)"/g)].map(match => match[1]);
  assert.ok(names.length >= 30);
  for (const name of names) assert.ok(commands.some(command => command.name === name), `Missing /${name}`);
  assert.ok(findCommands('reasoning').some(command => command.name === 'model'));
  assert.equal(parseCommand('/models')?.command.name, 'model');
  assert.equal(parseCommand('/unknown'), null);
  assert.equal(parseCommand('/remember line one\nline two')?.argument, 'line one\nline two');
});

test('commands route arguments as encoded data to appropriate screens', () => {
  const parsed = parseCommand('/artifacts');
  assert.ok(parsed);
  assert.equal(commandRoute(parsed.command, 'agt_test', ''), 'controls/agt_test?panel=artifacts&action=artifacts');
  const start = parseCommand('/new');
  assert.ok(start);
  assert.equal(commandRoute(start.command, 'agt_test', 'A & B'), 'new?panel=new&value=A+%26+B');
  const outbox=parseCommand('/outbox');
  assert.ok(outbox);
  assert.equal(commandRoute(outbox.command,'agt_test',''),'settings?panel=outbox');
});
