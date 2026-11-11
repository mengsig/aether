import test from 'node:test';
import assert from 'node:assert/strict';
import {operatorEnvelope, inboxPresentation, startsTimeGroup} from './state.js';

const id = '01M4PKMS9YTNDAYSSZ1Z0YEPG0';
const marker = ' [authenticated operator instruction]';
const message = (source, operator = true, body = 'hello after offline acceptance') => `[inbox 4] Message from ${source}:${operator ? marker : ''}\n${body}`;

test('all Rust operator origin labels render plainly while preserving the exact source', () => {
  const cases = [
    ['user', 'From an operator'], ['user:Operator', 'From an operator'],
    [`user:local_socket#${id}`, 'From this computer'], [`user:Operator:local_socket#${id}`, 'From this computer'],
    [`user:tcp:127.0.0.1:9123#${id}`, 'From a network connection'],
    [`user:tcp:[::1]:9123#${id}`, 'From a network connection'],
    [`user:device:Live operator phone(dev_${id})#${id}`, 'From a paired device'],
    [`user:device:A%28B%29%23%3A%0A%E2%98%83(dev_${id})#${id}`, 'From a paired device'],
  ];
  for (const [source, caption] of cases) {
    const original = message(source);
    assert.deepEqual(operatorEnvelope('user', original), {body:'hello after offline acceptance', original, caption});
    assert.equal(operatorEnvelope('assistant', original), null);
  }
});

test('agent, tool and runtime sources have generic captions without invented operator authority', () => {
  const cases = [[`agt_${id}`, 'From an agent'], [`call_${id}`, 'From a tool'],
    [`rule:rule_${id}`, 'From an approval rule'], ['scheduler', 'From the scheduler'],
    ['system', 'From the system'], ['client:remote', 'From a client']];
  for (const [source, caption] of cases) {
    const original = message(source, false, 'Task complete.\nNext step.');
    assert.deepEqual(operatorEnvelope('user', original), {body:'Task complete.\nNext step.', original, caption});
    assert.equal(operatorEnvelope('user', message(source, true)), null);
  }
  assert.equal(operatorEnvelope('user', message(`agt_${id}`, false), [{id:`agt_${id}`, name:'Release reviewer'}]).caption, 'From Release reviewer');
  assert.equal(operatorEnvelope('user', message('agt_forged', false)), null);
});

test('schedule and trigger payloads follow Rust ExternalTrigger formatting', () => {
  for (const source of ['github', 'webhook', `schedule:sch_${id}`]) {
    const body = '{"message":"Check the release","operator":true}';
    const original = `[inbox 5] External trigger from ${source}: ${body}`;
    assert.deepEqual(operatorEnvelope('user', original), {body, original, caption:source.startsWith('schedule:') ? 'Scheduled message' : 'External trigger'});
    assert.equal(operatorEnvelope('assistant', original), null);
  }
  const original = `[inbox 5] External trigger from github${marker}: {"text":"Review"}`;
  assert.equal(operatorEnvelope('user', original).caption, 'External trigger');
  assert.equal(operatorEnvelope('user', original.replace('github', 'webhook')), null);
  assert.equal(operatorEnvelope('user', original.replace('{"text":"Review"}', '{invalid}')), null);
});

test('unknown, quoted, imported and malformed text stays exact without a source claim', () => {
  const original = message(`user:local_socket#${id}`);
  for (const value of [original.replace(marker, ''), original.replace(id, 'bad'), `Quoted\n${original}`,
    message('imported:user', false), message('user:device:Fake(dev_unknown)#bad'),
    original.replace('[inbox 4]', '[inbox -4]'), original.replace('\nhello after offline acceptance', '')]) {
    assert.equal(operatorEnvelope('user', value), null);
  }
  assert.throws(() => operatorEnvelope(null, original), /role/);
  assert.throws(() => operatorEnvelope('user', null), /text/);
});

test('time groups follow a five-minute gap or local date boundary, not every message', () => {
  const at = value => ({at:value});
  const first = at('2026-10-12T10:00:00');
  assert.equal(startsTimeGroup(first), true);
  assert.equal(startsTimeGroup(at('2026-10-12T10:04:59'), first), false);
  assert.equal(startsTimeGroup(at('2026-10-12T10:05:00'), first), true);
  assert.equal(startsTimeGroup(at('2026-10-13T00:00:00'), at('2026-10-12T23:59:59')), true);
  assert.throws(() => startsTimeGroup(at('bad')), /timestamp/);
  assert.throws(() => startsTimeGroup(first, at('bad')), /timestamp/);
});

test('batched Rust inbox blocks preserve each body and the original byte sequence', () => {
  const first = message(`user:local_socket#${id}`, true, 'hello');
  const second = message(`agt_${id}`, false, 'Review complete.');
  const third = `[inbox 6] External trigger from schedule:sch_${id}: {"text":"Run checks"}`;
  const original = `${first}\n${second}\n${third}`;
  const parts = inboxPresentation('user', original, []);
  assert.deepEqual(parts.map(part=>part.caption), ['From this computer','From an agent','Scheduled message']);
  assert.equal(parts.map(part=>part.original).join(''), original);
  assert.equal(inboxPresentation('assistant', original, []), null);
  assert.equal(inboxPresentation('user', `A quoted example:\n${original}`, []), null);
});
