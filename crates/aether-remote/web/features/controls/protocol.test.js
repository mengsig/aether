import test from 'node:test';
import assert from 'node:assert/strict';
import { artifactBytes, assertDeviceSelf, integer, memoryCommand, modelReference, mutate, record, records,
  replyData, ruleCommand, scheduleCommand } from './protocol.js';
import { applicationKey } from './notifications.js';

const scheduleValues = { name: 'Daily check', kind: 'interval', seconds: '60', expression: '0 9 * * *',
  timezone: 'UTC', message: 'Check CI', busy: 'queue_one', missed: 'coalesce', limit: '' };

test('schedule forms produce an existing-agent target, never a spawn target', () => {
  const command = scheduleCommand(scheduleValues, 'agt_test');
  assert.deepEqual(command, { cmd: 'create_schedule', spec: { name: 'Daily check', target: 'agt_test',
    recurrence: { kind: 'interval', seconds: 60 }, payload: 'Check CI', busy: 'queue_one', missed: 'coalesce', max_deliveries: null } });
  const cron = scheduleCommand({ ...scheduleValues, kind: 'cron', limit: '2' }, 'agt_test');
  assert.deepEqual(cron.spec.recurrence, { kind: 'cron', expression: '0 9 * * *', timezone: 'UTC' });
  assert.equal(cron.spec.max_deliveries, 2);
});

test('invalid recurrence, counts and timezones are rejected before sending', () => {
  for (const seconds of ['0', '-1', '1.5', '1e3', '', '9007199254740993']) {
    assert.throws(() => scheduleCommand({ ...scheduleValues, seconds }, 'agt_test'));
  }
  assert.throws(() => scheduleCommand({ ...scheduleValues, kind: 'cron', expression: '* *' }, 'agt_test'), /five fields/);
  assert.throws(() => scheduleCommand({ ...scheduleValues, kind: 'cron', timezone: 'Not/AZone' }, 'agt_test'));
  assert.throws(() => scheduleCommand({ ...scheduleValues, busy: 'replace' }, 'agt_test'), /busy policy/);
  assert.throws(() => scheduleCommand(scheduleValues, ''), /Target agent/);
  assert.throws(() => integer(' 2 ', 'Count'));
});

test('memory forms preserve scope and do not fabricate provenance', () => {
  const values = { text: 'Keep local caches', kind: 'constraint', scope: 'agent', root: '', tags: 'build, cache, ' };
  const command = memoryCommand(values, 'agt_test');
  assert.deepEqual(command.spec, { text: values.text, kind: values.kind, scope: { scope: 'agent', id: 'agt_test' }, tags: ['build', 'cache'] });
  assert.deepEqual(memoryCommand({ ...values, scope: 'global' }, 'agt_test').spec.scope, { scope: 'global' });
  assert.deepEqual(memoryCommand({ ...values, scope: 'repository', root: '/work' }, 'agt_test').spec.scope,
    { scope: 'repository', root: '/work' });
  assert.throws(() => memoryCommand({ ...values, scope: 'repository' }, 'agt_test'), /Repository root/);
  assert.throws(() => memoryCommand({ ...values, text: ' ' }, 'agt_test'), /Memory is required/);
  assert.throws(() => memoryCommand({ ...values, kind: 'instruction' }, 'agt_test'), /kind/);
});

test('approval rule scopes and deliberate wildcard matcher use protocol fields', () => {
  const values = { scope: 'subtree', decision: 'denied', tool: '', argument: '', resource: '', note: '', run: 'run_test' };
  const command = ruleCommand(values, 'agt_test');
  assert.deepEqual(command.scope, { scope: 'subtree', root: 'agt_test' });
  assert.deepEqual(command.match, { tool: null, argument: null, resource_prefix: null });
  const scoped = ruleCommand({ ...values, scope: 'run', tool: 'shell', resource: '/work', decision: 'approved' }, 'agt_test');
  assert.deepEqual(scoped.scope, { scope: 'run', id: 'run_test' });
  assert.equal(scoped.match.resource_prefix, '/work');
  assert.throws(() => ruleCommand({ ...values, decision: 'allow' }, 'agt_test'), /decision/);
  assert.throws(() => ruleCommand({ ...values, scope: 'everywhere' }, 'agt_test'), /host access/);
});

test('model references split on the first slash and reject incomplete references', () => {
  assert.deepEqual(modelReference('provider/family/model'), { provider: 'provider', model: 'family/model' });
  for (const value of ['', 'model', '/model', 'provider/']) assert.throws(() => modelReference(value));
});

function contextHarness() {
  const state = { role: 'operator', connection: 'online' };
  const sent = [];
  const context = { store: { get: () => state }, confirm: async () => true,
    command: async command => { sent.push(command); return { reply: 'ok' }; } };
  return { state, sent, context };
}

test('observer, offline, declined and revoked-while-confirming operations never send', async () => {
  const { context, state, sent } = contextHarness();
  state.role = 'observer';
  await assert.rejects(mutate(context, { cmd: 'cancel' }), /read only/);
  state.role = 'operator';
  state.connection = 'offline';
  await assert.rejects(mutate(context, { cmd: 'cancel' }), /Reconnect/);
  state.connection = 'online';
  context.confirm = async () => false;
  assert.equal(await mutate(context, { cmd: 'cancel' }, 'Cancel?'), null);
  context.confirm = async () => { state.role = 'observer'; return true; };
  await assert.rejects(mutate(context, { cmd: 'cancel' }, 'Cancel?'), /read only/);
  assert.deepEqual(sent, []);
});

test('accepted mutations preserve the reply, and daemon failures remain failures', async () => {
  const { context, sent } = contextHarness();
  assert.deepEqual(await mutate(context, { cmd: 'pause', agent: 'agt_test', subtree: true }), { reply: 'ok' });
  assert.equal(sent.length, 1);
  context.command = async () => { throw new Error('Checkpoint failed'); };
  await assert.rejects(mutate(context, { cmd: 'restart' }), /Checkpoint failed/);
});

test('observer personal notifications remain available without granting fleet mutations', async () => {
  const { context, state, sent } = contextHarness();
  state.role = 'observer';
  assert.doesNotThrow(() => assertDeviceSelf(context));
  await assert.rejects(mutate(context, { cmd: 'pause' }), /read only/);
  state.connection = 'offline';
  assert.throws(() => assertDeviceSelf(context), /Reconnect/);
  assert.deepEqual(sent, []);
});

test('reply boundaries reject unexpected tags and malformed list records', () => {
  assert.throws(() => replyData({ reply: 'ok' }, 'memories'), /Expected memories/);
  assert.throws(() => replyData({ reply: 'memories' }, 'memories'), /no data/);
  assert.throws(() => record(null), /object/);
  assert.throws(() => record([]), /record/);
  assert.throws(() => records({}), /list/);
  assert.throws(() => records([null]), /object/);
  assert.deepEqual(records([{ id: 'one' }]), [{ id: 'one' }]);
});

test('artifact bytes preserve Unicode and truncation, reject malformed or oversized data', () => {
  const value = { artifact: { id: 'art_test' }, base64: Buffer.from('hello 🌍').toString('base64'), truncated: true };
  const result = artifactBytes(value);
  assert.equal(new TextDecoder().decode(result.bytes), 'hello 🌍');
  assert.equal(result.truncated, true);
  assert.throws(() => artifactBytes({ ...value, truncated: undefined }), /truncation/);
  assert.throws(() => artifactBytes({ ...value, base64: '!!!!' }));
  assert.throws(() => artifactBytes({ ...value, base64: Buffer.alloc(262145).toString('base64') }), /exceeds/);
});

test('push keys must represent an uncompressed P-256 public key', () => {
  const key = Buffer.alloc(65, 1);
  key[0] = 4;
  assert.deepEqual(applicationKey(key.toString('base64url')), Uint8Array.from(key));
  assert.throws(() => applicationKey(''), /required/);
  assert.throws(() => applicationKey('not!base64'), /encoding/);
  assert.throws(() => applicationKey(Buffer.alloc(65).toString('base64url')), /P-256/);
});
