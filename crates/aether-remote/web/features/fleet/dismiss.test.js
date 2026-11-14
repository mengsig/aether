import test from 'node:test';
import assert from 'node:assert/strict';
import { canDismiss, dismissCommands, failedItems } from './dismiss.js';

/** @typedef {import('../../types.js').Attention} Attention */
/** @param {Attention['kind']} kind @param {string} agentId @returns {Attention} */
function item(kind, agentId) {
  return { id: `attention-${kind}-${agentId}`, kind, agentId, title: kind, detail: '', at: '2026-11-14T00:00:00Z', raw: {} };
}

/** @param {'operator'|'observer'} role @returns {import('../../types.js').State} */
function state(role) {
  return /** @type {import('../../types.js').State} */ (/** @type {unknown} */ ({ role }));
}

test('only failed and blocked cards offer dismiss, and only to an operator', () => {
  const kinds = /** @type {Attention['kind'][]} */ (['approval', 'question', 'agent_failed', 'agent_blocked']);
  assert.deepEqual(kinds.filter(kind => canDismiss(state('operator'), item(kind, 'agt_1'))), ['agent_failed', 'agent_blocked']);
  assert.deepEqual(kinds.filter(kind => canDismiss(state('observer'), item(kind, 'agt_1'))), []);
});

test('a failed agent is archived; a blocked one is stopped first because the host archives only finished work', () => {
  assert.deepEqual(dismissCommands(item('agent_failed', 'agt_1')), [{ cmd: 'archive', target: { agent: 'agt_1' }, archived: true }]);
  assert.deepEqual(dismissCommands(item('agent_blocked', 'agt_2')).map(command => command.cmd), ['cancel', 'archive']);
  assert.equal(dismissCommands(item('agent_blocked', 'agt_2'))[0].agent, 'agt_2');
  assert.throws(() => dismissCommands(item('approval', 'agt_3')), /failed or blocked/);
});

test('clear all failed selects exactly the failed agents shown', () => {
  const attention = [item('agent_failed', 'agt_1'), item('agent_blocked', 'agt_2'), item('approval', 'agt_3'),
    item('question', 'agt_4'), item('agent_failed', 'agt_5')];
  assert.deepEqual(failedItems(attention).map(entry => entry.agentId), ['agt_1', 'agt_5']);
  assert.deepEqual(failedItems([]), []);
});
