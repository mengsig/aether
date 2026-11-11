import test from 'node:test';
import assert from 'node:assert/strict';
import {inboxRecord} from './inbox.js';
import {assertProtocol} from '../../dev/validate.js';
import {readCommand} from '../../dev/commands.js';
import {createFixtures} from '../../dev/fixtures.js';

test('resolved approval deliveries render the deciding actor and retain the exact record',()=>{
  const state=createFixtures();const reply=readCommand(state,{cmd:'get_inbox',agent:state.agents[0].id,after:0});
  assertProtocol('Reply',reply);const raw=reply.data[0];const item=inboxRecord(raw);
  assert.equal(item.title,'Approval granted');assert.equal(item.by,'Decided by: Operator Alex · Review phone');
  assert.equal(item.raw,raw);assert.equal(item.detail,'Reviewed the exact tool arguments.');
  assert.equal(inboxRecord({...raw,kind:{...raw.kind,by:undefined}}).by,'Decided by: Not reported');
  assert.throws(()=>inboxRecord({...raw,kind:{...raw.kind,decision:'allow'}}),/Invalid approval decision/);
  assert.throws(()=>inboxRecord({...raw,delivered_at:'not a timestamp'}),/delivery time/);
  const message={...raw,kind:{kind:'message',from:{actor:'system'},content:[{type:'text',text:''}]}};
  assert.equal(inboxRecord(message).detail,'');assert.equal(inboxRecord(message).by,'From: System');
});
