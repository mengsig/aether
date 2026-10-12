import { fakeLocks } from './lock-fixture.test.js';
import { withSubmissionLock, submissionLockName, assertSubmissionLock } from './submission-locks.js';
Object.defineProperty(globalThis.navigator,'locks',{configurable:true,value:fakeLocks()});
import test from 'node:test';
import assert from 'node:assert/strict';
import { createTransport } from './transport.js';
import { createStore } from './store.js';
import { readLedger } from './ledger.js';
import { markUnconfirmed } from './unconfirmed.js';
import { submissionResolution, welcomeHorizon } from './submissions.js';
import * as contract from './contract.js';
import * as identity from './identity.js';
import { createFixtures } from '../dev/fixtures.js';
import { readCommand } from '../dev/commands.js';
const retained_since = '2099-10-12T08:30:00Z';
const command = key => ({cmd:'send_message',to:'agt_00000000000000000000000002',content:'recover exactly',idempotency_key:key,delivery:{nested:['unchanged']}});
function storage() {
  const values = new Map();
  return {get length(){return values.size;},key:index=>[...values.keys()][index]??null,getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key)};
}
async function setup(horizon = 'first') {
  const disk = storage(); const frames = []; const wire = []; const fixtures = createFixtures();
  const env = {disk,wire,frames,horizon,lookup:()=>null,mutation:()=>null,beforeSend:()=>{},socket:null};
  const transport = createTransport({timeout:5000,random:()=>0,identity:{...identity,loadIdentity:async()=>({deviceId:'device',daemonId:'daemon',keys:{}}),saveIdentity:async()=>{}},
    adapter:{...contract,socketURL:()=> 'ws://test',authenticate:async()=>({cmd:'hello'})},socket:()=>{
      const socket = {readyState:1,onmessage:null,onclose:null,onerror:null,send(raw){
        const sent=JSON.parse(raw); env.beforeSend(sent); wire.push(sent);
        queueMicrotask(()=>{
          let reply;
          if(sent.cmd==='hello') reply={reply:'welcome',data:{protocol_version:18,instance_id:'daemon',device:{id:'device',name:'test',role:'operator'},...(env.horizon===null?{}:{submission_horizon:env.horizon})}};
          else if(sent.cmd==='subscribe') reply={reply:'subscribed',data:{last_seq:6}};
          else if(sent.cmd==='resolve_submission') reply=env.lookup(sent);
          else if(contract.isMutation(sent)) reply=env.mutation(sent);
          else reply=readCommand(fixtures,sent);
          if(reply) socket.reply(sent.req,reply);
        });
      },reply(id,reply){this.onmessage?.({data:JSON.stringify({frame:'reply',id,result:reply.Err?reply:{Ok:reply}})});},close(code=1000){this.readyState=3;this.onclose?.({code});}};
      env.socket=socket;queueMicrotask(()=>socket.onmessage({data:JSON.stringify({frame:'challenge',instance_id:'daemon',nonce:'A'.repeat(43)})}));return socket;
    }});
  const store=createStore(transport,{storage:disk});transport.onFrame(frame=>frames.push(frame));
  Object.assign(env,{transport,store,close:()=>{store.dispose();transport.disconnect();}});
  await transport.connect();await store.resolvePendingSubmissions();await tick();return env;
}
const tick = () => new Promise(resolve=>setImmediate(resolve));
async function lose(env, input) {
  const pending=env.transport.request(input);const rejected=assert.rejects(pending,/Outcome unknown/);
  await tick();env.transport.disconnect();await rejected;
  assert.equal(env.store.get().outbox.find(row=>row.id===input.idempotency_key).status,'unknown');
  assert.equal(readLedger(env.disk,'daemon','device').find(row=>row.id===input.idempotency_key).horizon,env.horizon);
}
async function reconnect(env) {await env.transport.connect();await env.store.resolvePendingSubmissions();await tick();}
function resolution(outcome, fields={}) {return {reply:'submission_resolved',data:{outcome,horizon:'later',retained_since,...fields}};}

test('executed lookup retires durable recovery without resending and preserves exact original reply',async()=>{
  const env=await setup();try {
    const original=command('executed');await lose(env,original);
    const receipt={reply:'message_sent',data:{message:'original-message',delivery:{sequence:27}}};
    env.lookup=()=>resolution('executed',{reply:receipt});await reconnect(env);
    const lookup=env.wire.find(row=>row.cmd==='resolve_submission');const {req,...payload}=lookup;
    assert.deepEqual(payload,{cmd:'resolve_submission',command:original,horizon:'first'});
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
    assert.deepEqual(env.store.get().outbox[0].reply,receipt);assert.equal(readLedger(env.disk,'daemon','device').length,0);
    assert.equal(env.frames.filter(frame=>frame.frame==='mutation'&&frame.command.cmd==='resolve_submission').length,0);
  }finally{env.close();}
});
test('absent_safe alone automatically resends exact original key and fields with immutable first horizon',async()=>{
  const env=await setup();try {
    const original=command('absent');await lose(env,original);env.horizon='welcome-new';
    env.lookup=()=>resolution('absent_safe');let persisted;
    env.mutation=()=>{persisted=readLedger(env.disk,'daemon','device')[0];return {reply:'message_sent',data:{message:'new-message'}};};
    await reconnect(env);
    const mutations=env.wire.filter(row=>row.cmd==='send_message').map(({req,...row})=>row);
    assert.deepEqual(mutations,[original,original]);assert.equal(persisted.horizon,'first');assert.deepEqual(persisted.command,original);
    assert.equal(readLedger(env.disk,'daemon','device').length,0);assert.equal(env.transport.submissionHorizon,'later');
    env.mutation=()=>null;const pending=env.transport.request(command('next'));const failed=assert.rejects(pending,/unknown/);
    await tick();assert.equal(readLedger(env.disk,'daemon','device')[0].horizon,'later');env.transport.disconnect();await failed;
  }finally{env.close();}
});
for(const reason of ['no_horizon','foreign_horizon','horizon_ahead','forgotten']) test(`unknown ${reason} never resends or changes original horizon`,async()=>{
  const env=await setup();try {
    await lose(env,command(reason));env.lookup=()=>resolution('unknown',{reason});await reconnect(env);
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
    const row=readLedger(env.disk,'daemon','device')[0];assert.equal(row.horizon,'first');assert.equal(row.recovery,'unknown');assert.equal(row.recoveryReason,reason);assert.match(row.error,/Delivery unconfirmed/);
  }finally{env.close();}
});
for(const code of ['invalid_argument','forbidden','not_found']) test(`resolver ${code} remains unconfirmed without mutation`,async()=>{
  const env=await setup();try {
    await lose(env,command(code));env.lookup=()=>({Err:{code,message:'lookup rejected'}});await reconnect(env);
    const row=readLedger(env.disk,'daemon','device')[0];assert.equal(row.recovery,'error');assert.match(row.error,/lookup rejected/);
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
  }finally{env.close();}
});
test('absent Welcome capability never calls resolver and legacy null horizon stays immutable after upgrade',async()=>{
  const env=await setup(null);try {
    await lose(env,command('legacy'));await reconnect(env);
    assert.equal(env.wire.filter(row=>row.cmd==='resolve_submission').length,0);assert.equal(env.store.get().outbox[0].recovery,'unsupported');
    env.transport.disconnect();env.horizon='upgraded';env.lookup=()=>resolution('unknown',{reason:'no_horizon'});await reconnect(env);
    const lookup=env.wire.find(row=>row.cmd==='resolve_submission');assert.equal(Object.hasOwn(lookup,'horizon'),false);
    assert.equal(readLedger(env.disk,'daemon','device')[0].horizon,null);
    assert.equal(welcomeHorizon({protocol_version:17,submission_horizon:'ignored'}),null);
    assert.equal(welcomeHorizon({protocol_version:18}),null);
  }finally{env.close();}
});
test('lost resolver retries after reconnect in creation order without replaying mutations',async()=>{
  const env=await setup();try {
    const first=env.transport.request(command('first'));const second=env.transport.request(command('second'));
    const rejected=[assert.rejects(first,/unknown/),assert.rejects(second,/unknown/)];await tick();env.transport.disconnect();await Promise.all(rejected);
    await env.transport.connect();await tick();assert.equal(env.wire.filter(row=>row.cmd==='resolve_submission').length,1);
    env.transport.disconnect();await tick();env.lookup=()=>resolution('executed',{reply:{reply:'ok'}});await reconnect(env);
    assert.deepEqual(env.wire.filter(row=>row.cmd==='resolve_submission').map(row=>row.command.idempotency_key),['first','first','second']);
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,2);assert.equal(readLedger(env.disk,'daemon','device').length,0);
  }finally{env.close();}
});
test('actual mutation Err retires durable pending and nested caller mutations cannot alter saved payload',async()=>{
  const env=await setup();try {
    const original=command('rejected');env.mutation=()=>({Err:{code:'invalid_argument',message:'bad input'}});
    const pending=env.transport.request(original);original.delivery.nested.push('later edit');
    await assert.rejects(pending,/bad input/);assert.equal(readLedger(env.disk,'daemon','device').length,0);
    assert.deepEqual(env.store.get().outbox[0].command.delivery.nested,['unchanged']);assert.equal(env.store.get().outbox[0].status,'failed');
  }finally{env.close();}
});
test('malformed resolver boundaries cannot authorize replay',()=>{
  for(const reply of [{reply:'ok'},resolution('other'),resolution('executed',{reply:{}}),resolution('absent_safe',{horizon:null}),resolution('unknown',{reason:'other'}),resolution('absent_safe',{retained_since:'yesterday'})]) assert.throws(()=>submissionResolution(reply));
  assert.throws(()=>welcomeHorizon({protocol_version:18,submission_horizon:null}));
  assert.throws(()=>contract.assertAllowed({cmd:'resolve_submission',command:command('key')},'observer'),/Observer/);
  assert.throws(()=>contract.assertAllowed({cmd:'resolve_submission',command:{cmd:'resolve_submission'}},'operator'),/keyed/);
});

test('actual late Ok or Err after timeout retires recovery on the original connection',async()=>{
  for(const result of [{reply:'ok'},{Err:{code:'invalid_argument',message:'late rejection'}}]) {
    const env=await setup();try {
      env.transport.options.timeout=5;env.lookup=()=>resolution('unknown',{reason:'forgotten'});
      await assert.rejects(env.transport.request(command('late')),/timed out/);await env.store.resolvePendingSubmissions();
      assert.equal(readLedger(env.disk,'daemon','device').length,1);
      env.socket.reply(env.wire.find(row=>row.cmd==='send_message').req,result);
      await tick();assert.equal(readLedger(env.disk,'daemon','device').length,0);assert.equal(env.store.get().outbox[0].status,result.Err?'failed':'sent');
    }finally{env.close();}
  }
});
test('cross-tab ledger preserves immutable payload and horizon and cannot resurrect retired rows',async()=>{
  const env=await setup();try {
    await lose(env,command('shared'));
    const other=createStore(env.transport,{storage:env.disk});
    try {
      await other.receive({frame:'mutation',phase:'pending',id:'shared',command:command('shared'),horizon:'new-tab'});
      assert.equal(other.get().outbox[0].horizon,'first');
      await other.receive({frame:'mutation',phase:'sent',id:'shared',command:command('shared'),reply:{reply:'ok'}});
      env.store.patch({outbox:env.store.get().outbox.map(row=>({...row,recovery:'error'}))});
      assert.equal(readLedger(env.disk,'daemon','device').length,0);
      env.lookup=()=>resolution('absent_safe');await reconnect(env);
      assert.equal(env.wire.filter(row=>row.cmd==='resolve_submission').length,0);
    }finally{other.dispose();}
  }finally{env.close();}
});
test('a second tab cannot reuse a pending key with different fields',async()=>{
  const env=await setup();try {
    await lose(env,command('shared'));
    const other=createStore(env.transport,{storage:env.disk});try {
      await assert.rejects(other.receive({frame:'mutation',phase:'pending',id:'shared',command:{...command('shared'),content:'changed'}}),/preserve/);
      assert.equal(readLedger(env.disk,'daemon','device')[0].command.content,'recover exactly');
    }finally{other.dispose();}
  }finally{env.close();}
});
test('malformed resolver reply remains unconfirmed without mutation replay',async()=>{
  const env=await setup();try {
    await lose(env,command('malformed'));env.lookup=()=>resolution('absent_safe',{horizon:42});await reconnect(env);
    assert.equal(env.transport.submissionHorizon,'first');assert.equal(env.store.get().outbox[0].recovery,'error');
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
  }finally{env.close();}
});

test('executed receipt remains authoritative when a late original Err races lookup',async()=>{
  const env=await setup();try {
    env.lookup=()=>resolution('executed',{reply:{reply:'message_sent',data:{message:'committed'}}});
    env.transport.options.timeout=5;await assert.rejects(env.transport.request(command('race')),/timed out/);
    const req=env.wire.find(row=>row.cmd==='send_message').req;
    env.lookup=()=>resolution('executed',{reply:{reply:'message_sent',data:{message:'committed'}}});
    await env.store.resolvePendingSubmissions();env.socket.reply(req,{Err:{code:'invalid_argument',message:'late error'}});
    assert.equal(env.store.get().outbox[0].status,'sent');assert.equal(env.store.get().outbox[0].reply.data.message,'committed');
    assert.equal(readLedger(env.disk,'daemon','device').length,0);
  }finally{env.close();}
});
test('operator retry after unknown resolution preserves first horizon and original payload',async()=>{
  const env=await setup();try {
    await lose(env,command('manual'));env.lookup=()=>resolution('unknown',{reason:'forgotten'});await reconnect(env);
    env.mutation=()=>null;const pending=env.transport.request(env.store.get().outbox[0].command);const rejected=assert.rejects(pending,/unknown/);
    await tick();assert.equal(readLedger(env.disk,'daemon','device')[0].horizon,'first');
    assert.deepEqual(readLedger(env.disk,'daemon','device')[0].command,command('manual'));
    env.transport.disconnect();await rejected;
  }finally{env.close();}
});

test('lost lookup timeout reconnects and retries only the readonly lookup',async()=>{
  const env=await setup();try {
    await lose(env,command('lookup-timeout'));env.transport.options.timeout=5;
    await reconnect(env);assert.equal(env.transport.status,'reconnecting');
    assert.equal(env.store.get().outbox[0].recovery,'resolving');assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
    env.lookup=()=>resolution('executed',{reply:{reply:'ok'}});await reconnect(env);
    assert.equal(env.wire.filter(row=>row.cmd==='resolve_submission').length,2);assert.equal(readLedger(env.disk,'daemon','device').length,0);
  }finally{env.close();}
});

test('first keyed mutation stores exact command and horizon before the socket sees bytes',async()=>{
  const env=await setup();try {
    const original=command('durable-before-wire');let checked=false;
    env.beforeSend=sent=>{
      if(sent.cmd!=='send_message')return;
      const row=readLedger(env.disk,'daemon','device')[0];assert.deepEqual(row.command,original);assert.equal(row.horizon,'first');
      assert.equal(row.status,'pending');assert.equal(Object.hasOwn(row.command,'req'),false);checked=true;
    };
    env.mutation=()=>({reply:'ok'});await env.transport.request(original);
    assert.equal(checked,true);assert.equal(readLedger(env.disk,'daemon','device').length,0);
  }finally{env.close();}
});
test('capability absence on a later Welcome clears remembered support and never resolves',async()=>{
  const env=await setup();try {
    await lose(env,command('downgrade'));env.horizon=null;await reconnect(env);
    assert.equal(env.transport.submissionHorizon,null);assert.equal(env.store.get().outbox[0].recovery,'unsupported');
    assert.equal(env.wire.filter(row=>row.cmd==='resolve_submission').length,0);assert.equal(readLedger(env.disk,'daemon','device')[0].horizon,'first');
  }finally{env.close();}
});

test('an explicit retry after actual Err recreates a durable record before sending',async()=>{
  const env=await setup();try {
    env.mutation=()=>({Err:{code:'invalid_argument',message:'first rejected'}});
    await assert.rejects(env.transport.request(command('retry-rejected')),/first rejected/);assert.equal(readLedger(env.disk,'daemon','device').length,0);
    env.beforeSend=sent=>{if(sent.cmd==='send_message')assert.deepEqual(readLedger(env.disk,'daemon','device')[0].command,command('retry-rejected'));};
    env.mutation=()=>({reply:'ok'});await env.transport.request(command('retry-rejected'));
    assert.equal(env.store.get().outbox[0].status,'sent');
  }finally{env.close();}
});
test('a stale tab cannot send after another tab reconciled the durable record',async()=>{
  const env=await setup();try {
    await lose(env,command('reconciled'));
    const other=createStore(env.transport,{storage:env.disk});try{await other.dismissReceipt('reconciled');}finally{other.dispose();}
    env.transport.status='online';await assert.rejects(env.transport.request(command('reconciled')),/already/);
    assert.equal(readLedger(env.disk,'daemon','device').length,0);assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
  }finally{env.close();}
});

test('a mutation timeout on a healthy socket immediately starts readonly recovery',async()=>{
  const env=await setup();try {
    env.transport.options.timeout=5;env.lookup=()=>resolution('unknown',{reason:'forgotten'});
    await assert.rejects(env.transport.request(command('healthy-timeout')),/timed out/);await env.store.resolvePendingSubmissions();
    assert.equal(env.transport.status,'online');assert.equal(env.store.get().outbox[0].recovery,'unknown');
    assert.equal(env.wire.filter(row=>row.cmd==='resolve_submission').length,1);assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
  }finally{env.close();}
});
test('another tab settlement keeps the originating receipt through timeout without a false preflight failure',async()=>{
  for(const status of ['sent','dismissed']) {
    const env=await setup();try {
      env.transport.options.timeout=10;const pending=env.transport.request(command('other-tab'));const rejected=assert.rejects(pending,/Outcome unknown/);await tick();
      const other=createStore(env.transport,{storage:env.disk});try {
        if(status==='sent')await other.receive({frame:'mutation',phase:'sent',id:'other-tab',command:command('other-tab'),reply:{reply:'ok'}});
        else await other.dismissReceipt('other-tab');
      }finally{other.dispose();}
      env.store.externalStorage({key:'aether.remote.receipt.v1:daemon:device:other-tab',newValue:null});
      assert.equal(env.store.get().outbox[0].status,status);await rejected;
      assert.equal(env.store.get().outbox[0].status,status);assert.equal(env.store.get().error,null);assert.equal(readLedger(env.disk,'daemon','device').length,0);
    }finally{env.close();}
  }
});
test('late original Err cannot retire a newer same-key wire attempt whose reply is lost',async()=>{
  const env=await setup();try {
    env.transport.options.timeout=5;env.lookup=()=>resolution('unknown',{reason:'forgotten'});
    await assert.rejects(env.transport.request(command('newer-wire')),/timed out/);await env.store.resolvePendingSubmissions();
    const originalRequest=env.wire.find(row=>row.cmd==='send_message').req;env.transport.options.timeout=5000;
    const pending=env.transport.request(command('newer-wire'));const rejected=assert.rejects(pending,/Outcome unknown/);
    await tick();env.socket.reply(originalRequest,{Err:{code:'invalid_argument',message:'old attempt rejected'}});
    assert.equal(env.store.get().outbox[0].status,'pending');assert.equal(readLedger(env.disk,'daemon','device').length,1);
    env.transport.disconnect();await rejected;
    assert.equal(env.store.get().outbox[0].status,'unknown');assert.equal(readLedger(env.disk,'daemon','device').length,1);
  }finally{env.close();}
});
test('each initial persistence failure leaves no hidden command for later automatic recovery',async()=>{
  for(const failedWrite of [1,2]) {
    const env=await setup();try {
      let writes=0;const set=env.disk.setItem;
      env.disk.setItem=(key,value)=>{if(++writes===failedWrite)throw new Error('quota fault');set(key,value);};
      await assert.rejects(env.transport.request(command('partial-write')),/quota fault/);
      assert.equal(env.store.get().outbox.length,0);assert.equal(readLedger(env.disk,'daemon','device').length,0);
      assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,0);
      env.disk.setItem=set;env.transport.disconnect();env.lookup=()=>resolution('absent_safe');await reconnect(env);
      assert.equal(env.wire.filter(row=>row.cmd==='resolve_submission').length,0);assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,0);
    }finally{env.close();}
  }
});
test('timeouts arriving during a lookup get one followup pass without rechecking explicit unknowns',async()=>{
  const env=await setup();try {
    env.transport.options.timeout=50;
    await assert.rejects(env.transport.request(command('first-arrival')),/Outcome unknown/);
    await env.store.recovering;env.transport.options.timeout=5000;
    const recovering=env.store.resolvePendingSubmissions('first-arrival');
    await tick();const lookup=env.wire.find(row=>row.cmd==='resolve_submission');assert.ok(lookup);
    env.transport.options.timeout=5;
    await assert.rejects(env.transport.request(command('second-arrival')),/Outcome unknown/);
    env.lookup=()=>resolution('unknown',{reason:'forgotten'});
    env.socket.reply(lookup.req,resolution('unknown',{reason:'forgotten'}));
    await recovering;
    assert.deepEqual(env.wire.filter(row=>row.cmd==='resolve_submission').map(row=>row.command.idempotency_key),['first-arrival','second-arrival']);
    assert.ok(env.store.get().outbox.every(row=>row.recovery==='unknown'));
    assert.equal(env.store.recovering,null);assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,2);
  }finally{env.close();}
});
test('a rejected safe resend preserves an older uncertain wire without a retry loop',async()=>{
  const env=await setup();try {
    await lose(env,command('older-uncertain'));env.lookup=()=>resolution('absent_safe');
    env.mutation=()=>({Err:{code:'forbidden',message:'New send rejected'}});
    await reconnect(env);
    assert.equal(env.wire.filter(row=>row.cmd==='resolve_submission').length,1);
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,2);
    assert.equal(env.store.get().outbox[0].status,'unknown');assert.equal(env.store.get().outbox[0].recovery,'error');
    assert.equal(readLedger(env.disk,'daemon','device').length,1);
  }finally{env.close();}
});
for(const initial of ['horizon_ahead','unsupported','forbidden']) test(`permanent ${initial} uncertainty survives reload and later absent_safe`,async()=>{
  const env=await setup();try {
    await lose(env,command(`latched-${initial}`));
    env.horizon=initial==='unsupported'?null:'first';
    env.lookup=()=>initial==='forbidden'?{Err:{code:'forbidden',message:'Lookup refused'}}:resolution('unknown',{reason:'horizon_ahead'});
    await reconnect(env);assert.equal(env.store.get().outbox[0].unconfirmed.reason,initial);
    const restored=createStore(env.transport,{storage:env.disk});
    assert.equal(restored.get().outbox[0].unconfirmed.reason,initial);restored.dispose();
    env.transport.disconnect();env.horizon='new-welcome';env.lookup=()=>resolution('absent_safe');await reconnect(env);
    assert.equal(env.store.get().outbox[0].status,'unknown');assert.equal(env.store.get().outbox[0].horizon,'first');
    assert.equal(env.store.get().outbox[0].unconfirmed.reason,initial);
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
    env.transport.disconnect();env.lookup=()=>resolution('executed',{reply:{reply:'message_sent',data:{message:'retained'}}});await reconnect(env);
    assert.equal(env.store.get().outbox[0].status,'sent');assert.equal(readLedger(env.disk,'daemon','device').length,0);
  }finally{env.close();}
});
test('a late cross-tab uncertainty marker blocks safe resend even during pending persistence',async()=>{
  const env=await setup();try {
    await lose(env,command('cross-tab-latch'));env.lookup=()=>resolution('absent_safe');
    const set=env.disk.setItem;let latched=false;
    env.disk.setItem=(key,value)=>{
      if(!latched&&key.startsWith('aether.remote.receipt.')&&JSON.parse(value).status==='pending') {
        latched=true;markUnconfirmed(env.disk,'daemon','device','cross-tab-latch',{reason:'horizon_ahead',message:'Another tab could not confirm delivery.'});
      }
      set(key,value);
    };
    await reconnect(env);assert.equal(latched,true);
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
    assert.equal(env.store.get().outbox[0].unconfirmed.reason,'horizon_ahead');
    assert.equal(readLedger(env.disk,'daemon','device')[0].unconfirmed.reason,'horizon_ahead');
  }finally{env.close();}
});
test('an explicit send-again decision is durable and a lost acknowledgement keeps its uncertainty latch',async()=>{
  const env=await setup();try {
    const original=command('manual-latched');await lose(env,original);
    env.lookup=()=>resolution('unknown',{reason:'horizon_ahead'});await reconnect(env);
    const retried=env.transport.request(original);const rejected=assert.rejects(retried,/Outcome unknown/);await tick();
    assert.equal(readLedger(env.disk,'daemon','device')[0].decision,'send_again');
    assert.equal(readLedger(env.disk,'daemon','device')[0].unconfirmed.reason,'horizon_ahead');
    env.transport.disconnect();await rejected;
    env.lookup=()=>resolution('absent_safe');await reconnect(env);
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,2);
    assert.equal(env.store.get().outbox[0].horizon,'first');
    assert.equal(env.store.get().outbox[0].unconfirmed.reason,'horizon_ahead');
  }finally{env.close();}
});

test('mark delivered remains recorded after late original response', async () => {
  const env = await setup();
  try {
    env.transport.options.timeout = 5;
    env.lookup = () => resolution('unknown', {reason:'forgotten'});
    await assert.rejects(env.transport.request(command('decision')), /timed out/);
    await env.store.resolvePendingSubmissions();
    const other = createStore(env.transport, {storage:env.disk});
    await other.dismissReceipt('decision', 'delivered');
    other.dispose();
    const key = 'aether.remote.retired.v1:daemon:device:decision:dismissed';
    assert.equal(env.disk.getItem(key), 'delivered');
    env.socket.reply(env.wire.find(row => row.cmd === 'send_message').req, {reply:'ok'});
    assert.equal(env.disk.getItem(key), 'delivered');
  } finally { env.close(); }
});

test('blocked automatic resend does not leave an unsent wire after original Err', async () => {
  const env = await setup();
  try {
    env.transport.options.timeout = 5;
    env.lookup = () => resolution('absent_safe');
    const set = env.disk.setItem;
    let pendingWrites = 0;
    env.disk.setItem = (key, value) => {
      if (key.startsWith('aether.remote.receipt.') && JSON.parse(value).status === 'pending' && ++pendingWrites === 2) {
        markUnconfirmed(env.disk, 'daemon', 'device', 'ghost', {reason:'horizon_ahead', message:'Other tab uncertainty'});
      }
      set(key, value);
    };
    await assert.rejects(env.transport.request(command('ghost')), /timed out/);
    await env.store.resolvePendingSubmissions();
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    env.socket.reply(env.wire.find(row => row.cmd === 'send_message').req, {Err:{code:'forbidden', message:'original rejected'}});
    await tick();assert.equal(readLedger(env.disk, 'daemon', 'device').length, 0);
  } finally { env.close(); }
});

test('lookup completion does not turn an explicit in-flight retry back into unknown', async () => {
  const env = await setup();
  try {
    await lose(env, command('manual-race'));
    env.lookup = () => resolution('unknown', {reason:'forgotten'});
    await reconnect(env);
    env.lookup = () => null;
    const recovering = env.store.resolvePendingSubmissions();
    await tick();
    const lookup = env.wire.filter(row => row.cmd === 'resolve_submission').at(-1);
    const retry = env.transport.request(command('manual-race'));
    const rejected = assert.rejects(retry, /Outcome unknown/);
    assert.equal(env.store.get().outbox[0].status, 'unknown');
    env.socket.reply(lookup.req, resolution('absent_safe'));
    await recovering;await tick();
    const status = env.store.get().outbox[0].status;
    env.transport.disconnect();
    await rejected;
    assert.equal(status, 'pending');
  } finally { env.close(); }
});

test('uncertainty stays latched in memory when marker persistence fails', async () => {
  const env = await setup();
  try {
    await lose(env, command('quota-latch'));
    const set = env.disk.setItem;
    env.disk.setItem = (key, value) => {
      if (key.startsWith('aether.remote.unconfirmed.')) throw new Error('marker quota fault');
      set(key, value);
    };
    env.lookup = () => resolution('unknown', {reason:'horizon_ahead'});
    await assert.rejects(reconnect(env), /marker quota fault/);
    assert.match(env.store.get().error, /marker quota fault/);
    env.disk.setItem = set;
    env.transport.disconnect();
    env.lookup = () => resolution('absent_safe');
    env.mutation = () => ({reply:'ok'});
    await reconnect(env);
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
  } finally { env.close(); }
});

test('post-persistence marker read failure compensates only the unsent wire', async () => {
  const env = await setup();
  try {
    env.transport.options.timeout = 5;
    env.lookup = () => resolution('absent_safe');
    const set = env.disk.setItem;
    const get = env.disk.getItem;
    let pendingWrites = 0;
    let failRead = false;
    env.disk.setItem = (key, value) => {
      set(key, value);
      if (key.startsWith('aether.remote.receipt.') && JSON.parse(value).status === 'pending' && ++pendingWrites === 2) failRead = true;
    };
    env.disk.getItem = key => {
      if (failRead && key.startsWith('aether.remote.unconfirmed.')) {
        failRead = false;
        throw new Error('post-write marker read failed');
      }
      return get(key);
    };
    await assert.rejects(env.transport.request(command('read-fault')), /timed out/);
    await env.store.resolvePendingSubmissions();
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    const original = env.wire.find(row => row.cmd === 'send_message');
    env.socket.reply(original.req, {Err:{code:'forbidden', message:'original rejected'}});
    await tick();assert.equal(readLedger(env.disk, 'daemon', 'device').length, 0);
  } finally { env.close(); }
});

test('marker write fallback preserves original uncertainty across store reload', async () => {
  const env = await setup();
  let restored;
  try {
    await lose(env, command('quota-reload'));
    const set = env.disk.setItem;
    env.disk.setItem = (key, value) => {
      if (key.startsWith('aether.remote.unconfirmed.')) throw new Error('marker quota fault');
      set(key, value);
    };
    env.lookup = () => resolution('unknown', {reason:'horizon_ahead'});
    await assert.rejects(reconnect(env), /marker quota fault/);
    assert.equal(readLedger(env.disk, 'daemon', 'device')[0].unconfirmed.reason, 'horizon_ahead');
    env.disk.setItem = set;
    env.transport.disconnect();
    env.store.dispose();
    restored = createStore(env.transport, {storage:env.disk});
    assert.equal(restored.get().outbox[0].unconfirmed.reason, 'horizon_ahead');
    env.lookup = () => resolution('absent_safe');
    await env.transport.connect();
    await restored.resolvePendingSubmissions();
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
  } finally { restored?.dispose(); env.close(); }
});
test('total write outage keeps known uncertainty through same-tab welcome after storage recovers',async()=>{
  const env=await setup();try {
    await lose(env,command('all-write-quota'));const set=env.disk.setItem;
    env.lookup=()=>{env.disk.setItem=()=>{throw new Error('All writes refused');};return resolution('unknown',{reason:'horizon_ahead'});};
    await assert.rejects(reconnect(env),/All writes refused/);
    assert.equal(env.store.get().outbox[0].unconfirmed.reason,'horizon_ahead');
    env.disk.setItem=set;env.transport.disconnect();env.lookup=()=>resolution('absent_safe');await reconnect(env);
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
    assert.equal(readLedger(env.disk,'daemon','device')[0].unconfirmed.reason,'horizon_ahead');
  }finally{env.close();}
});

test('missing Web Locks permanently disables safe automatic resend but permits explicit send again', async () => {
  const locks = navigator.locks;
  Object.defineProperty(navigator, 'locks', {configurable:true, value:undefined});
  const env = await setup();
  try {
    const original = command('no-locks'); await lose(env, original);
    env.lookup = () => resolution('absent_safe'); await reconnect(env);
    const row = readLedger(env.disk, 'daemon', 'device')[0];
    assert.equal(row.unconfirmed.reason, 'locks_unavailable');
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    env.mutation = () => ({reply:'ok'});
    let decision;
    env.beforeSend = sent => { if (sent.cmd === 'send_message') decision = readLedger(env.disk, 'daemon', 'device')[0]; };
    await env.transport.request(original);
    assert.equal(decision.decision, 'send_again'); assert.equal(decision.horizon, 'first');
    assert.equal(decision.unconfirmed.reason, 'locks_unavailable'); assert.deepEqual(decision.command, original);
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 2);
  } finally { env.close(); Object.defineProperty(navigator, 'locks', {configurable:true, value:locks}); }
});

test('reentrant submission leases reject a different key, different scope, and expired ownership', async () => {
  const scope = {daemonId:'daemon', deviceId:'device'}; let expired;
  await withSubmissionLock(scope, 'lease', async lease => {
    expired = lease; assert.ok(Object.isFrozen(lease));
    assert.equal(await withSubmissionLock(scope, 'lease', held => held === lease, {lease}), true);
    assert.throws(() => assertSubmissionLock(lease, scope, 'other'), /another receipt/);
    assert.throws(() => assertSubmissionLock(lease, {...scope,deviceId:'other'}, 'lease'), /another receipt/);
  });
  assert.throws(() => withSubmissionLock(scope, 'lease', () => {}, {lease:expired}), /no longer active/);
  assert.notEqual(submissionLockName(scope, 'lease'), submissionLockName({...scope,daemonId:'other'}, 'lease'));
});

async function holdSubmission(id) {
  let release; let entered;
  const ready = new Promise(resolve => { entered = resolve; });
  const gate = new Promise(resolve => { release = resolve; });
  const done = withSubmissionLock({daemonId:'daemon',deviceId:'device'}, id, async () => { entered(); await gate; });
  await ready; return {release, done};
}

for (const outcome of ['sent', 'dismissed']) test(`queued explicit retry rechecks ${outcome} retirement before sending`, async () => {
  const env = await setup(); let other; let held;
  try {
    await lose(env, command('queued-terminal'));
    env.transport.status = 'online'; env.store.patch({connection:'online'});
    other = createStore(env.transport, {storage:env.disk});
    held = await holdSubmission('queued-terminal');
    const settle = outcome === 'sent'
      ? other.receive({frame:'mutation',phase:'sent',id:'queued-terminal',command:command('queued-terminal'),reply:{reply:'ok'}})
      : other.dismissReceipt('queued-terminal');
    const retry = env.transport.request(command('queued-terminal'));
    const rejected = assert.rejects(retry, /already/);
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    held.release(); await held.done; await settle; await rejected;
    assert.equal(readLedger(env.disk, 'daemon', 'device').length, 0);
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
  } finally { held?.release(); other?.dispose(); env.close(); }
});

for (const change of ['disconnect', 'role', 'session']) test(`queued manual retry cancels after ${change}`, async () => {
  const env = await setup(); let held;
  try {
    await lose(env, command('queue-cancel')); env.transport.status = 'online';
    held = await holdSubmission('queue-cancel');
    const retry = env.transport.request(command('queue-cancel'));
    const rejected = assert.rejects(retry, /Connection changed|Observer/);
    if (change === 'disconnect') env.transport.disconnect();
    if (change === 'role') env.transport.role = 'observer';
    if (change === 'session') env.transport.submissionScope = {daemonId:'different',deviceId:'device'};
    held.release(); await held.done; await rejected;
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    assert.equal(readLedger(env.disk, 'daemon', 'device')[0].horizon, 'first');
  } finally { held?.release(); env.close(); }
});

test('deferred recovery cancels on disconnect and a new epoch resolves after the lock releases', async () => {
  const env = await setup(); let held;
  try {
    await lose(env, command('queue-recover')); held = await holdSubmission('queue-recover');
    env.lookup = () => resolution('executed', {reply:{reply:'ok'}});
    await env.transport.connect(); const obsolete = env.store.resolvePendingSubmissions(); await tick();
    assert.equal(env.wire.filter(row => row.cmd === 'resolve_submission').length, 0);
    env.transport.disconnect(); await obsolete;
    await env.transport.connect(); const current = env.store.resolvePendingSubmissions();
    held.release(); await held.done; await current;
    await env.store.resolvePendingSubmissions('queue-recover');
    assert.equal(env.wire.filter(row => row.cmd === 'resolve_submission').length, 1);
    assert.equal(readLedger(env.disk, 'daemon', 'device').length, 0);
  } finally { held?.release(); env.close(); }
});

test('a competing unknown lookup cannot enter between the final latch read and wire send', async () => {
  const env = await setup(); let other; let competing; let lookups = 0;
  try {
    await lose(env, command('final-read-race'));
    other = createStore({onFrame:()=>()=>{},onConnection:()=>()=>{},request:async()=>{
      lookups++; return resolution('unknown', {reason:'horizon_ahead'});
    }}, {storage:env.disk});
    other.patch({connection:'online',role:'operator'}); other.submissionLookup = true;
    const send = env.transport.send.bind(env.transport); let checked = false;
    env.transport.send = async (input, sendId) => {
      if (input.cmd === 'send_message') {
        assert.equal(readLedger(env.disk, 'daemon', 'device')[0].status, 'pending');
        competing = other.resolvePendingSubmissions(); await tick();
        assert.equal(lookups, 0); assert.equal(readLedger(env.disk, 'daemon', 'device')[0].unconfirmed, undefined);
        checked = true;
      }
      return send(input, sendId);
    };
    env.lookup = () => resolution('absent_safe'); env.mutation = () => ({reply:'ok'});
    await reconnect(env); await competing;
    assert.equal(checked, true); assert.equal(lookups, 0);
    assert.equal(readLedger(env.disk, 'daemon', 'device').length, 0);
  } finally { other?.dispose(); env.close(); }
});

test('a held first key defers one timer while later keys resolve without a busy loop', async t => {
  const env = await setup(); let held;
  try {
    const results = ['held-first','free-second'].map(id => assert.rejects(env.transport.request(command(id)), /unknown/));
    await tick(); env.transport.disconnect(); await Promise.all(results);
    held = await holdSubmission('held-first'); t.mock.timers.enable({apis:['setTimeout']});
    env.lookup = () => resolution('executed', {reply:{reply:'ok'}}); await reconnect(env);
    assert.deepEqual(env.wire.filter(row => row.cmd === 'resolve_submission').map(row => row.command.idempotency_key), ['free-second']);
    assert.deepEqual([...env.store.deferredRecovery], ['held-first']);
    const timer = env.store.recoveryTimer; assert.notEqual(timer, undefined);
    await tick(); assert.equal(env.store.recoveryTimer, timer);
    t.mock.timers.tick(999); await tick();
    assert.equal(env.wire.filter(row => row.cmd === 'resolve_submission').length, 1);
    held.release(); await held.done; t.mock.timers.tick(1); await tick();
    assert.deepEqual(env.wire.filter(row => row.cmd === 'resolve_submission').map(row => row.command.idempotency_key), ['free-second','held-first']);
    assert.equal(env.store.recoveryTimer, undefined); assert.equal(env.store.deferredRecovery.size, 0);
  } finally { held?.release(); env.close(); }
});

for (const action of ['disconnect','dispose']) test(`deferred recovery timer is cleaned on ${action}`, async t => {
  const env = await setup(); let held;
  try {
    await lose(env, command('timer-cleanup')); held = await holdSubmission('timer-cleanup');
    t.mock.timers.enable({apis:['setTimeout']}); await reconnect(env);
    assert.notEqual(env.store.recoveryTimer, undefined); assert.equal(env.store.deferredRecovery.size, 1);
    if (action === 'disconnect') env.transport.disconnect(); else env.store.dispose();
    assert.equal(env.store.recoveryTimer, undefined); assert.equal(env.store.deferredRecovery.size, 0);
    held.release(); await held.done; t.mock.timers.tick(10_000); await tick();
    assert.equal(env.wire.filter(row => row.cmd === 'resolve_submission').length, 0);
  } finally { held?.release(); env.close(); }
});

for (const action of ['retry','dismiss']) test(`manual ${action} has a bounded lock wait and never runs after timeout`, async t => {
  const env = await setup(); let held;
  try {
    await lose(env, command('manual-busy')); held = await holdSubmission('manual-busy');
    env.transport.status = 'online'; t.mock.timers.enable({apis:['setTimeout']});
    const waiting = action === 'retry' ? env.transport.request(command('manual-busy')) : env.store.dismissReceipt('manual-busy');
    let finished = false;
    const rejected = assert.rejects(waiting, /Another tab is still working on this delivery/).then(() => { finished = true; });
    t.mock.timers.tick(2999); await tick(); assert.equal(finished, false);
    t.mock.timers.tick(1); await rejected;
    held.release(); await held.done; await tick();
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    assert.equal(readLedger(env.disk, 'daemon', 'device').length, 1);
    assert.equal(env.disk.getItem('aether.remote.retired.v1:daemon:device:manual-busy:dismissed'), null);
  } finally { held?.release(); env.close(); }
});

test('callback persistence errors do not disable Web Locks or execute callbacks twice', async () => {
  const scope = {daemonId:'daemon',deviceId:'device'}; let calls = 0;
  await assert.rejects(withSubmissionLock(scope, 'storage-error', () => { calls++; throw new DOMException('Quota reached', 'SecurityError'); }), /Quota reached/);
  await withSubmissionLock(scope, 'storage-error', lease => { calls++; assert.equal(lease.exclusive, true); });
  assert.equal(calls, 2);
});

test('rejected native lock requests latch unavailable until reload without rerunning callbacks', async () => {
  const env = await setup(); const locks = navigator.locks; let requests = 0;
  try {
    await lose(env, command('denied-locks'));
    Object.defineProperty(navigator, 'locks', {configurable:true, value:{request:async () => {
      requests++; throw new DOMException('Storage access denied', 'SecurityError');
    }}});
    env.lookup = () => resolution('absent_safe'); await reconnect(env);
    assert.equal(requests, 1); assert.equal(env.store.get().outbox[0].unconfirmed.reason, 'locks_unavailable');
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    env.mutation = () => ({reply:'ok'}); await env.transport.request(command('denied-locks'));
    assert.equal(requests, 1); assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 2);
    assert.equal(readLedger(env.disk, 'daemon', 'device').length, 0);
  } finally { env.close(); Object.defineProperty(navigator, 'locks', {configurable:true, value:locks}); }
});
