import test from 'node:test';
import assert from 'node:assert/strict';
import { createTransport } from './transport.js';
import * as contract from './contract.js';
import * as identity from './identity.js';
import { frame } from './validate.js';
class FakeSocket {
  readyState = 0; sent = []; onopen = null; onmessage = null; onclose = null; onerror = null;
  send(raw) {
    const value = JSON.parse(raw); this.sent.push(value);
    if (value.cmd === 'hello') queueMicrotask(()=>this.reply(value.req,{reply:'welcome',data:{daemon_version:'test',protocol_version:18,instance_id:'daemon',device:{id:'device',name:'test',role:this.role}}}));
  }
  reply(id, value) { this.onmessage?.({data:JSON.stringify({frame:'reply',id,result:{Ok:value}})}); }
  close(code = 1000) { this.readyState = 3; this.onclose?.({code}); }
}
async function connected(role = 'operator') {
  const socket = new FakeSocket(); socket.role = role;
  const transport = createTransport({socket:()=>{queueMicrotask(()=>{socket.readyState=1;socket.onopen?.();socket.onmessage?.({data:JSON.stringify({frame:'challenge',instance_id:'daemon',nonce:'A'.repeat(43)})});});return socket;},
    adapter:{...contract,authenticate:async()=> ({cmd:'hello',protocol_version:18}),socketURL:()=> 'ws://localhost/test'},
    identity:{...identity,saveIdentity:async()=>{},loadIdentity:async()=>({deviceId:'device',daemonId:'daemon',role,name:'test',keys:{}})},timeout:1000});
  await transport.connect(); return {socket,transport};
}
test('request correlation cannot be overwritten by command IDs or req',async()=>{
  const {socket,transport} = await connected();
  try {
    const first = transport.request({cmd:'get_agent',id:'agt_a',req:999}); const second = transport.request({cmd:'get_agent',id:'agt_b'});
    const [a,b] = socket.sent.slice(-2); assert.notEqual(a.req,999); assert.equal(a.id,'agt_a');
    socket.reply(b.req,{reply:'agent',data:{id:'agt_b'}}); socket.reply(a.req,{reply:'agent',data:{id:'agt_a'}});
    assert.equal((await first).data.id,'agt_a'); assert.equal((await second).data.id,'agt_b');
  } finally { transport.disconnect(); }
});
test('pending writes become unknown on close and are never replayed',async()=>{
  const {socket,transport} = await connected();
  const pending = transport.request({cmd:'send_message',to:'agt_a',content:'hi',idempotency_key:'stable'});
  const check = assert.rejects(pending,error=>error.outcome === 'unknown');
  transport.disconnect(); await check; assert.equal(socket.sent.filter(item=>item.cmd==='send_message').length,1);
});
test('all finalized keyed mutations receive distinct keys and preserve an explicit retry key',async()=>{
  const {socket,transport}=await connected();
  const commands=['create_agent','create_task','send_message','reply_to_question','trigger','create_schedule','resolve_approval','restart',
    'cancel','interrupt','pause','resume','archive','update_agent','set_approvals','update_schedule','create_memory','add_approval_rule'];
  const keys=new Set();
  try {
    for(const cmd of commands){
      const input={cmd,...(cmd==='trigger'?{source:'phone'}:cmd==='update_agent'?{patch:{}}:{})};const pending=transport.request(input);const sent=socket.sent.at(-1);
      assert.match(sent.idempotency_key,/^[0-9a-f-]{36}$/);keys.add(sent.idempotency_key);
      assert.equal(input.idempotency_key,undefined);socket.reply(sent.req,{reply:'ok'});await pending;
      const retry=transport.request({...input,idempotency_key:sent.idempotency_key});const repeated=socket.sent.at(-1);
      assert.equal(repeated.idempotency_key,sent.idempotency_key);socket.reply(repeated.req,{reply:'ok'});await retry;
    }
    assert.equal(keys.size,18);
    for(const cmd of ['compact','wake','forget_memory','revoke_approval_rule','subscribe_push']){
      const pending=transport.request({cmd});const sent=socket.sent.at(-1);
      assert.equal(sent.idempotency_key,undefined);socket.reply(sent.req,{reply:'ok'});await pending;
    }
  } finally {transport.disconnect();}
});
test('observer guard is enforced at transport boundary',async()=>{
  const {socket,transport} = await connected('observer');
  try {
    await assert.rejects(transport.request({cmd:'pause',agent:'agt_a'}),/Observer/);
    await assert.rejects(transport.request({cmd:'mcp_servers',refresh:true}),/Observer/);
    assert.equal(socket.sent.length,1); assert.equal(contract.isMutation({cmd:'mcp_servers',refresh:false}),false);
  } finally { transport.disconnect(); }
});
test('invalid server frame closes connection and rejects pending operations',async()=>{
  const {socket,transport} = await connected(); const statuses=[]; transport.onConnection(status=>statuses.push(status));
  const pending = transport.request({cmd:'get_agent',id:'agt_a'}); const rejected=assert.rejects(pending,/Unknown server frame/);
  socket.onmessage({data:'{"frame":"unexpected"}'}); await rejected;
  assert.equal(socket.readyState,3); assert.equal(statuses.at(-1),'offline'); transport.disconnect();
});
test('strict frame validation rejects unsafe sequences and ambiguous results',()=>{
  assert.throws(()=>frame(JSON.stringify({frame:'reply',id:1,result:{Ok:{reply:'ok'},Err:{code:'bad',message:'bad'}}})),/exactly one/);
  assert.throws(()=>frame(JSON.stringify({frame:'event',sub:1,event:{seq:2**53,at:'today',kind:{type:'test'}}})),/safe/);
  assert.throws(()=>frame(JSON.stringify({frame:'delta',sub:1,agent:'a',invocation:'i',delta:{delta:'text',text:42}})),/text/);
});
test('device key generation is nonextractable P-256',async()=>{
  const device = await identity.newIdentity();
  assert.equal(device.keys.privateKey.extractable,false); assert.equal(device.keys.privateKey.algorithm.namedCurve,'P-256');
  await assert.rejects(crypto.subtle.exportKey('jwk',device.keys.privateKey));
  assert.equal((await crypto.subtle.exportKey('jwk',device.keys.publicKey)).crv,'P-256');
});
test('receipt persistence failure prevents mutation from reaching the socket',async()=>{
  const {createStore}=await import('./store.js'); const {socket,transport}=await connected();
  let fail=false;const storage={getItem:()=>null,setItem:()=>{if(fail)throw new Error('Storage quota reached');}};
  const store=createStore(transport,{storage});store.deviceId='device';store.patch({daemonId:'daemon',connection:'online',role:'operator'});fail=true;
  try {
    await assert.rejects(transport.request({cmd:'send_message',to:'agt_00000000000000000000000002',content:'do not lose me'}),/quota/);
    assert.equal(socket.sent.filter(item=>item.cmd==='send_message').length,0);assert.equal(store.get().outbox.length,0);
  } finally {store.dispose();transport.disconnect();}
});
test('role change reconnects but authentication refusal stops retries',async()=>{
  const {socket,transport}=await connected();const statuses=[];transport.onConnection(status=>statuses.push(status));
  socket.close(4403);assert.equal(statuses.at(-1),'reconnecting');transport.disconnect();
  const second=await connected();const refused=[];second.transport.onConnection(status=>refused.push(status));
  second.socket.close(4401);assert.equal(refused.at(-1),'revoked');assert.equal(second.transport.stopped,true);second.transport.disconnect();
});
test('operator filesystem reads neither emit receipts nor bypass observer guard',async()=>{
  const reads=['complete_path','get_git_summary','validate_workspace'];
  const {socket,transport}=await connected();const frames=[];transport.onFrame(frame=>frames.push(frame));
  try {
    for(const cmd of reads){
      assert.equal(contract.isMutation({cmd}),false);
      assert.throws(()=>contract.assertAllowed({cmd},'observer'),/Observer/);
      const pending=transport.request({cmd,prefix:'/',workspace:'/work',path:'/work'});socket.reply(socket.sent.at(-1).req,{reply:'ok'});await pending;
    }
    assert.equal(frames.filter(frame=>frame.frame==='mutation').length,0);
  } finally {transport.disconnect();}
});

test('displaced sockets back off without losing their device pairing',async()=>{
  const {socket,transport}=await connected();const notices=[];
  transport.onConnection((status,detail)=>notices.push({status,detail}));
  try {
    socket.close(4408);
    assert.equal(transport.stopped,false);assert.ok(transport.attempt>=3);
    assert.equal(notices.at(-1).status,'reconnecting');assert.match(notices.at(-1).detail,/timed out/);
  } finally {transport.disconnect();}
});

test('full session sets preserve the pairing and capacity reason while backing off',async()=>{
  const {socket,transport}=await connected();transport.disconnect();const notices=[];
  transport.onConnection((status,detail)=>notices.push({status,detail}));
  socket.send=raw=>{
    const command=JSON.parse(raw);socket.sent.push(command);
    queueMicrotask(()=>socket.onmessage({data:JSON.stringify({frame:'reply',id:command.req,result:{Err:{code:'unavailable',message:'every device connection is in use'}}})}));
  };
  try {
    await assert.rejects(transport.connect(),/every device connection/);
    assert.equal(transport.stopped,false);assert.ok(transport.attempt>=4);
    assert.equal(notices.at(-1).status,'reconnecting');assert.match(notices.at(-1).detail,/All device connections/);
  } finally {transport.disconnect();}
});
test('pairing stays unpaired, never offline, and ignores reconnect attempts until it settles',async()=>{
  const statuses=[]; let loads=0; let approve=()=>{}; let refuse=()=>{};
  const transport = createTransport({socket:()=>{throw new Error('pairing must not open a socket');},
    adapter:{...contract,pairDevice:()=>new Promise((resolve,reject)=>{approve=resolve;refuse=reject;})},
    identity:{...identity,newIdentity:async()=>({keys:{}}),saveIdentity:async()=>{},loadIdentity:async()=>{loads+=1;return undefined;}}});
  transport.onConnection(status=>statuses.push(status));
  const pairing = transport.pair('ABCD-1234','Phone');
  await transport.connect(); assert.equal(loads,0); assert.deepEqual(statuses,['unpaired']);
  refuse(new Error('Pairing rejected')); await assert.rejects(pairing,/rejected/);
  assert.deepEqual(statuses,['unpaired','unpaired']);
  await transport.connect(); assert.equal(loads,1); assert.equal(statuses.at(-1),'unpaired');
});
test('a forget that lands while pairing saves its identity leaves nothing stored',async()=>{
  let stored=false; let saving=()=>{}; const started=new Promise(resolve=>{saving=resolve;}); let finishSave=()=>{};
  const transport = createTransport({socket:()=>{throw new Error('a forgotten pairing must not connect');},
    adapter:{...contract,pairDevice:async identity=>({...identity,deviceId:'device'})},
    identity:{...identity,newIdentity:async()=>({keys:{}}),loadIdentity:async()=>undefined,deleteIdentity:async()=>{stored=false;},
      saveIdentity:()=>new Promise(resolve=>{saving();finishSave=()=>{stored=true;resolve();};})}});
  const pairing = transport.pair('ABCD-1234','Phone'); await started;
  const forgetting = transport.forget(); finishSave();
  await assert.rejects(pairing,/cancelled/); await forgetting;
  assert.equal(stored,false); assert.equal(transport.status,'unpaired');
});
