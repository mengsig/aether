import test from 'node:test';
import assert from 'node:assert/strict';
import {requestAbort,checkAbort} from './abort.js';
test('request cancellation forwards external abort and handles already aborted signals',()=>{
  const external=new AbortController();const request=requestAbort(1000,external.signal);
  const reason=new Error('Cancelled by operator');external.abort(reason);
  assert.equal(request.signal.reason,reason);assert.throws(()=>checkAbort(request.signal),/operator/);request.cleanup();
  const second=requestAbort(1000,external.signal);assert.equal(second.signal.aborted,true);second.cleanup();
  assert.throws(()=>requestAbort(0),/timeout/);
});
test('request timeout is bounded and cleanup removes cancellation resources',async()=>{
  const request=requestAbort(5);await new Promise(resolve=>setTimeout(resolve,15));
  assert.equal(request.signal.aborted,true);assert.match(request.signal.reason.message,/timed out/);request.cleanup();
  const external=new AbortController();const cleaned=requestAbort(5,external.signal);cleaned.cleanup();external.abort();
  await new Promise(resolve=>setTimeout(resolve,15));assert.equal(cleaned.signal.aborted,false);
});
