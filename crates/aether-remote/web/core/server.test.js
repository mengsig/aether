import test from 'node:test';
import assert from 'node:assert/strict';
import { WebSocket } from 'ws';
import { once } from 'node:events';
import { connect } from 'node:net';
import { createMockServer } from '../dev/server.js';
import { newIdentity, base64url } from './identity.js';
import { authenticate, assertAllowed } from './contract.js';
async function device(auth) {
  const identity=await newIdentity();
  const public_key=base64url(new Uint8Array(await crypto.subtle.exportKey('raw',identity.keys.publicKey)));
  const claim=auth.claim({code:'aether-demo',device_name:'Test browser',public_key});
  assert.equal(auth.status({pairing:claim.pairing}).state,'claimed');
  auth.confirm(claim.pairing,true); const enrolled=auth.status({pairing:claim.pairing});
  return {...identity,deviceId:enrolled.device.id};
}
test('mock requires host enrollment and verifies single-use origin-bound P-256 proof',async()=>{
  const server=createMockServer();
  try {
    const identity=await device(server.auth); const challenge=server.auth.challenge('https://host.test');
    const hello=await authenticate(identity,challenge,'https://host.test');
    assert.equal(server.auth.authenticate(hello.device,challenge).id,identity.deviceId);
    assert.throws(()=>server.auth.authenticate(hello.device,challenge),/expired/);
    const next=server.auth.challenge('https://host.test'); const wrong=await authenticate(identity,next,'https://attacker.test');
    assert.throws(()=>server.auth.authenticate(wrong.device,next),/signature/);
    const expired=server.auth.challenge('https://host.test'); server.auth.challenges.set(expired.nonce,0);
    assert.throws(()=>server.auth.authenticate(hello.device,expired),/expired/);
    await assert.rejects(authenticate({...identity,daemonId:'different'},challenge,'https://host.test'),/identity changed/);
  } finally { await server.close(); }
});
test('remote matrix blocks host administration and allows observer own push',()=>{
  for(const cmd of ['revoke_device','shutdown','get_remote']) assert.throws(()=>assertAllowed({cmd},'operator'),/host/);
  assert.doesNotThrow(()=>assertAllowed({cmd:'subscribe_push'},'observer'));
  assert.throws(()=>assertAllowed({cmd:'complete_path'},'observer'),/Observer/);
  assert.throws(()=>assertAllowed({cmd:'add_approval_rule',scope:{scope:'everywhere'}},'operator'),/host/);
});
test('keyed creation returns its original receipt and changed payload conflicts',async()=>{
  const server=createMockServer();
  try {
    const identity=await device(server.auth);const session={device:server.auth.device(identity.deviceId),hello:true};
    const command={cmd:'create_memory',spec:{text:'Preserve operator decisions',kind:'fact'},idempotency_key:'memory-once'};
    const before=server.state.memories.length;const first=server.execute(command,session,1);
    server.state.memories.at(-1).text='Changed after the first receipt';
    const duplicate=server.execute({idempotency_key:command.idempotency_key,spec:command.spec,cmd:command.cmd},session,2);
    assert.equal(duplicate.data.id,first.data.id);assert.equal(duplicate.data.text,'Preserve operator decisions');
    assert.equal(server.state.memories.length,before+1);
    assert.throws(()=>server.execute({...command,spec:{...command.spec,text:'Changed content'}},session,3),/different command/);
    assert.equal(server.state.memories.length,before+1);
  } finally {await server.close();}
});
test('mock HTTP restricts private files, origin, and serves strict CSP',async()=>{
  const server=createMockServer(); server.http.listen(0,'127.0.0.1'); await once(server.http,'listening');
  const base=`http://127.0.0.1:${server.http.address().port}`;
  try {
    const page=await fetch(base); assert.equal(page.status,200); assert.match(page.headers.get('content-security-policy'),/object-src 'none'/);
    assert.equal((await fetch(`${base}/node_modules/ws/package.json`)).status,400);
    assert.equal((await fetch(`${base}/dev/auth.js`)).status,400);
    assert.equal((await fetch(`${base}/api/pair/claim`,{method:'POST',headers:{'Content-Type':'application/json','Origin':'https://evil.example'},body:'{}'})).status,403);
    assert.equal((await (await fetch(`${base}/api/health`)).json()).protocol_version,18);
  } finally { await server.close(); }
});
test('closing the mock drops a connection that never sent a request',async()=>{
  const server=createMockServer(); server.http.listen(0,'127.0.0.1'); await once(server.http,'listening');
  const accepted=once(server.http,'connection'); const socket=connect(server.http.address().port,'127.0.0.1');
  await accepted; const closing=server.close();
  try {
    await assert.doesNotReject(once(socket,'close',{signal:AbortSignal.timeout(2000)}),'the mock left an unused connection open');
    assert.equal(socket.bytesWritten,0);
  } finally { socket.destroy(); await closing; }
});
test('authenticated mock socket challenges first and uses daemon envelope and replay cursor',async()=>{
  const server=createMockServer(); server.http.listen(0,'127.0.0.1'); await once(server.http,'listening');
  const base=`http://127.0.0.1:${server.http.address().port}`; const identity=await device(server.auth);
  const ws=new WebSocket(`${base.replace('http:','ws:')}/ws`,'aether',{origin:base});
  try {
    const incoming=once(ws,'message'); await once(ws,'open');
    const challenge=JSON.parse(String((await incoming)[0])); assert.equal(challenge.frame,'challenge');
    const hello=await authenticate(identity,challenge,base); const welcome=once(ws,'message');
    ws.send(JSON.stringify({req:1,...hello}));
    assert.equal(JSON.parse(String((await welcome)[0])).result.Ok.reply,'welcome');
    const frames=[]; ws.on('message',data=>frames.push(JSON.parse(String(data))));
    const event=new Promise(resolve=>ws.on('message',data=>{const frame=JSON.parse(String(data));if(frame.frame==='event')resolve(frame);}));
    ws.send(JSON.stringify({req:2,cmd:'subscribe',after:0,query:{},deltas:true}));
    assert.equal((await event).event.seq,6); assert.equal(frames[0].result.Ok.reply,'subscribed');
  } finally {ws.terminate();await server.close();}
});
