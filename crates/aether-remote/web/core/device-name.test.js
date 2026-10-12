import test from 'node:test';
import assert from 'node:assert/strict';
import {normalizeDeviceName,pairDevice} from './contract.js';
import {newIdentity} from './identity.js';

test('device names preserve visible scripts and count Unicode characters after whitespace folding',()=>{
  for (const name of ['Robin’s iPhone','Zoë','ケンの電話','كمبيوتر','e\u0301cran','📱'.repeat(64)]) assert.equal(normalizeDeviceName(name),name);
  assert.equal(normalizeDeviceName(' \u00a0Phone\u2028\u2029\u2009 tablet\u3000 '),'Phone tablet');
  assert.equal(normalizeDeviceName('  '+ 'a'.repeat(64) +'  '),'a'.repeat(64));
  for (const name of ['', ' \u00a0 ','x'.repeat(65),'📱'.repeat(65)]) assert.throws(()=>normalizeDeviceName(name),/Device name/);
});

test('invisible, control, private and noncharacter code points are refused before trimming',()=>{
  const hidden=['\t','\n','\u0085','\u00ad','\u034f','\u061c','\u200b','\u200d','\u202e','\u2066',
    '\ufe0f','\ufeff','\u115f','\u3164','\u2800','\ufffc','\ue000','\u{e0061}','\u{e0100}','\u{f0000}','\u{10ffff}','\u{1fffe}','\ud800'];
  for (const point of hidden) for (const name of [point+'Phone','Phone'+point,'Ph'+point+'one']) assert.throws(()=>normalizeDeviceName(name),/cannot contain/);
});

test('pairing refuses an invalid name before key export or a network request',async()=>{
  await assert.rejects(pairDevice({keys:{}},'ABCD-1234','Phone\ufeff'),/cannot contain/);
});

test('pairing sends normalized names and explains the bounded Retry-After cooldown',async t=>{
  const identity=await newIdentity();let sent;
  t.mock.method(globalThis,'fetch',async(_url,options)=>{
    sent=JSON.parse(options.body);
    return new Response(JSON.stringify({error:{code:'rate_limited',message:'too many guesses'}}),{status:429,headers:{'Retry-After':'1'}});
  });
  await assert.rejects(pairDevice(identity,'ABCD-1234','  My\u00a0 phone  '),error=>error.code==='rate_limited' && error.retryAfter===1 && /Wait 1 second/.test(error.message));
  assert.equal(sent.device_name,'My phone');assert.equal(sent.code,'ABCD-1234');
});
