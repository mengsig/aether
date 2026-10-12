import test from 'node:test';
import assert from 'node:assert/strict';
import { MockSubmissions } from '../dev/submissions.js';
const operator={id:'device-one',role:'operator'};
const command={cmd:'create_task',objective:'Exact original task',idempotency_key:'task-once'};
test('mock resolver checks authority, caller namespace and complete original payload',()=>{
  const receipts=new Map();const mock=new MockSubmissions(receipts);const outer={cmd:'resolve_submission',command,horizon:mock.horizon};
  const reply={reply:'task',data:{id:'task-one',objective:'Exact original task'}};
  receipts.set('device-one:task-once',{command,reply});
  const executed=mock.resolve(outer,operator);assert.deepEqual(executed.data.reply,reply);assert.equal(executed.data.outcome,'executed');
  assert.equal(mock.resolve(outer,{...operator,id:'device-two'}).data.outcome,'absent_safe');
  assert.throws(()=>mock.resolve(outer,{...operator,role:'observer'}),/Observer/);
  assert.throws(()=>mock.resolve({...outer,command:{...command,objective:'Changed'}},operator),/different command/);
  executed.data.reply.data.objective='client change';assert.equal(receipts.get('device-one:task-once').reply.data.objective,'Exact original task');
});
test('mock ledger distinguishes all horizon absence outcomes without wall clock inference',()=>{
  const mock=new MockSubmissions(new Map());const resolve=horizon=>mock.resolve({cmd:'resolve_submission',command,...(horizon?{horizon}:{})},operator).data;
  assert.equal(resolve(mock.horizon).outcome,'absent_safe');assert.equal(resolve(null).reason,'no_horizon');
  assert.equal(resolve('foreign:1').reason,'foreign_horizon');assert.equal(resolve('mock-submissions:99').reason,'horizon_ahead');
  const old=mock.horizon;mock.configure({cmd:'mock_submission_fault',forget:true});assert.equal(resolve(old).reason,'forgotten');
  assert.throws(()=>resolve('broken token'),/Malformed/);
  assert.equal(resolve(mock.horizon).outcome,'absent_safe');
});
