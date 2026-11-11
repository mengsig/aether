import test from 'node:test';
import assert from 'node:assert/strict';
import {finishedAgent} from './composer-input.js';

test('terminal agent status disables delivery only for the selected conversation', () => {
  for (const status of ['completed', 'failed', 'cancelled']) {
    const state = {agents:[{id:'selected', status}, {id:'running', status:'running'}]};
    assert.equal(finishedAgent(state, 'selected'), true);
    assert.equal(finishedAgent(state, 'running'), false);
  }
  for (const status of ['running', 'ready', 'waiting', 'blocked']) {
    assert.equal(finishedAgent({agents:[{id:'selected', status}]}, 'selected'), false);
  }
  assert.equal(finishedAgent({agents:[]}, 'missing'), false);
});
