import test from 'node:test';
import assert from 'node:assert/strict';
import { createStore } from './store.js';
import { createCommand } from './command.js';
import { applyEvent } from './events.js';
import { createDrafts } from './persistence.js';
import { readCommand } from '../dev/commands.js';
import { createFixtures } from '../dev/fixtures.js';
function mock() {
  const fixtures=createFixtures(); let listener;
  const transport={onFrame:fn=>{listener=fn;return()=>{};},onConnection:()=>()=>{},
    request:async command=>readCommand(fixtures,command)};
  return {transport,emit:frame=>listener(frame)};
}
test('store normalizes raw state and preserves unknown coverage',async()=>{
  const {transport}=mock(); const store=createStore(transport,{storage:null});
  try {
    await store.refresh(); assert.equal(store.get().agents.length,7); assert.equal(store.get().agents[0].model,'codex/gpt-6-astra');
    assert.equal(store.get().agents[5].costComplete,false); assert.equal(store.get().attention[0].raw.side_effect,'external');
    await store.loadTranscript('agt_00000000000000000000000002'); assert.equal(store.get().transcripts.agt_00000000000000000000000002.length,5);
  } finally {store.dispose();}
});
test('duplicate replay has no repeated effects and invalid application does not advance cursor',()=>{
  const {transport,emit}=mock(); const store=createStore(transport,{storage:null});
  try {
    const event={seq:7,at:'now',agent_id:'agt_00000000000000000000000002',kind:{type:'question.asked',question:'Ship it?'}};
    emit({frame:'event',event}); emit({frame:'event',event});
    assert.equal(store.get().lastSeq,7); assert.equal(store.get().attention.length,1); assert.equal(store.get().activity.length,1);
    assert.throws(()=>applyEvent(store.get(),{...event,seq:8,kind:{type:'agent.message',role:'assistant',content:null}}));
    assert.equal(store.get().lastSeq,7);
  } finally {store.dispose();}
});
test('durable pending receipt reloads unknown and same-key retry reuses its row',()=>{
  const values=new Map(); const storage={get length(){return values.size;},key:index=>[...values.keys()][index]??null,getItem:key=>values.get(key),setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key)};
  const {transport,emit}=mock(); const store=createStore(transport,{storage});
  const command={cmd:'send_message',to:'agt_00000000000000000000000002',content:'hello',idempotency_key:'stable'};
  store.patch({daemonId:'daemon'}); emit({frame:'mutation',phase:'pending',id:'stable',command});
  assert.equal(JSON.parse([...values.entries()].find(([key])=>key.startsWith('aether.remote.receipt.v1:'))[1]).command.content,'hello'); store.dispose();
  const reloaded=mock(); const restored=createStore(reloaded.transport,{storage});
  try {
    assert.equal(restored.get().outbox[0].status,'unknown');
    reloaded.emit({frame:'mutation',phase:'pending',id:'stable',command});
    assert.equal(restored.get().outbox.length,1); assert.equal(restored.get().outbox[0].status,'pending');
    reloaded.emit({frame:'mutation',phase:'sent',id:'stable',command}); assert.equal(restored.get().outbox[0].status,'sent');
    assert.throws(()=>reloaded.emit({frame:'mutation',phase:'pending',id:'stable',command}),/already/);
  } finally {restored.dispose();}
});
test('expired cursor resync buffers concurrent events and commits cursor after snapshots',async()=>{
  const {transport,emit}=mock(); const requests=[]; const original=transport.request;
  transport.request=async command=>{
    requests.push(command);
    if(command.cmd!=='subscribe') return original(command);
    if(command.after!==null) throw Object.assign(new Error('History expired'),{code:'not_found'});
    emit({frame:'event',event:{seq:8,at:'now',agent_id:'agt_00000000000000000000000002',kind:{type:'agent.message',role:'assistant',content:[{type:'text',text:'While loading'}]}}});
    return {reply:'subscribed',data:{id:1,last_seq:6}};
  };
  const store=createStore(transport,{storage:null}); store.patch({lastSeq:200,connection:'online',transcripts:{agt_00000000000000000000000002:[]}});
  try {
    await store.startSubscription(); assert.deepEqual(requests.filter(item=>item.cmd==='subscribe').map(item=>item.after),[200,null]);
    assert.equal(store.get().lastSeq,8); assert.equal(store.get().transcripts.agt_00000000000000000000000002.at(-1).message.content[0].text,'While loading');
  } finally {store.dispose();}
});
test('draft persistence is bounded and storage failure does not silently discard current draft',()=>{
  const saved=new Map(); const storage={getItem:key=>saved.get(key),setItem:(key,value)=>saved.set(key,value)};
  const drafts=createDrafts(storage);
  for(let index=0;index<60;index++) drafts.set(`agent-${index}`,'draft');
  assert.equal(Object.keys(JSON.parse(saved.values().next().value)).length,50); assert.equal(drafts.get('agent-59'),'draft');
  storage.setItem=()=>{throw new Error('quota exceeded');}; assert.throws(()=>drafts.set('agent-59','changed'),/quota/);
  assert.equal(drafts.get('agent-59'),'draft'); assert.throws(()=>drafts.set('agent-59','x'.repeat(20001)),/limited/);
});

test('a second tab checkpoint cannot overwrite another tab unresolved receipt',()=>{
  const values=new Map(); const storage={get length(){return values.size;},key:index=>[...values.keys()][index]??null,getItem:key=>values.get(key),setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key)};
  const first=mock();const second=mock();const a=createStore(first.transport,{storage});const b=createStore(second.transport,{storage});
  try {
    a.patch({daemonId:'daemon'}); b.patch({daemonId:'daemon'});
    first.emit({frame:'mutation',phase:'pending',id:'first',command:{cmd:'pause',agent:'agt_00000000000000000000000001'}});
    b.patch({lastSeq:12,activity:[]});
    second.emit({frame:'mutation',phase:'pending',id:'second',command:{cmd:'resume',agent:'agt_00000000000000000000000002'}});
    a.patch({lastSeq:13});
    const reloaded=createStore(mock().transport,{storage});
    assert.deepEqual(new Set(reloaded.get().outbox.map(row=>row.id)),new Set(['first','second']));
    assert.ok(reloaded.get().outbox.every(row=>row.status==='unknown'));reloaded.dispose();
  } finally {a.dispose();b.dispose();}
});
test('explicit receipt dismissal persists across reload and rejects pending commands',async()=>{
  const values=new Map();const storage={get length(){return values.size;},key:index=>[...values.keys()][index]??null,getItem:key=>values.get(key),setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key)};
  const first=mock();const store=createStore(first.transport,{storage});store.patch({daemonId:'daemon'});
  const command={cmd:'pause',agent:'agt_00000000000000000000000001'};
  first.emit({frame:'mutation',phase:'pending',id:'review',command});
  await assert.rejects(store.dismissReceipt('review'),/pending/);assert.equal(store.get().outbox.length,1);
  first.emit({frame:'mutation',phase:'unknown',id:'review',command});await store.dismissReceipt('review');
  assert.equal(store.get().outbox.length,0);store.dispose();
  const reloaded=createStore(mock().transport,{storage});assert.equal(reloaded.get().outbox.length,0);reloaded.dispose();
});

test('messages for unopened agents cannot evict the transcript being read',async()=>{
  const {transport,emit}=mock();const store=createStore(transport,{storage:null});
  const reading='agt_00000000000000000000000002';await store.loadTranscript(reading);const retained=store.get().transcripts[reading];
  try {
    for(let index=0;index<30;index++) emit({frame:'event',event:{seq:100+index,at:'now',agent_id:`agent-${index}`,kind:{type:'agent.message',role:'assistant',content:[{type:'text',text:'Another agent'}]}}});
    assert.equal(store.get().transcripts[reading],retained);assert.equal(Object.keys(store.get().transcripts).length,1);
    assert.equal(store.get().activity.length,30);
  } finally {store.dispose();}
});
test('completed transcript refresh uses its held cursor after retention',async()=>{
  const {transport}=mock();const original=transport.request;const requests=[];let first=true;
  transport.request=async command=>{
    if(command.cmd!=='get_transcript')return original(command);requests.push(command.after);
    if(!first&&command.after===0)throw Object.assign(new Error('History expired'),{code:'not_found'});
    const seq=first?10:11;first=false;return {reply:'transcript',data:[{seq,at:'now',message:{role:'assistant',content:[{type:'text',text:`Entry ${seq}`}]}}]};
  };
  const store=createStore(transport,{storage:null});
  try {await store.loadTranscript('reading');await store.loadTranscript('reading');assert.deepEqual(requests,[0,10]);assert.deepEqual(store.get().transcripts.reading.map(row=>row.seq),[10,11]);}
  finally {store.dispose();}
});
test('failed initial transcript remains incomplete and expired conversation does not block resync',async()=>{
  const {transport,emit}=mock();const original=transport.request;const requests=[];
  transport.request=async command=>{
    if(command.cmd==='subscribe')return {reply:'subscribed',data:{id:1,last_seq:6}};
    if(command.cmd!=='get_transcript')return original(command);requests.push(command.after);
    throw Object.assign(new Error('History expired'),{code:'not_found'});
  };
  const store=createStore(transport,{storage:null});
  try {
    await assert.rejects(store.loadTranscript('reading'),/expired/);
    emit({frame:'event',event:{seq:10,at:'now',agent_id:'reading',kind:{type:'agent.message',role:'assistant',content:[{type:'text',text:'Live message'}]}}});
    await store.fullResync(store.epoch);assert.deepEqual(requests,[0,0]);assert.equal(store.get().ready,true);
    assert.match(store.get().error,/reading.*expired/);assert.equal(store.get().transcripts.reading[0].seq,10);
  } finally {store.dispose();}
});

test('receipt eviction marks unresolved keyed deliveries and preserves the warning durably',()=>{
  const values=new Map();const storage={get length(){return values.size;},key:index=>[...values.keys()][index]??null,getItem:key=>values.get(key),setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key)};
  const {transport,emit}=mock();const store=createStore(transport,{storage});store.patch({daemonId:'daemon'});
  const command={cmd:'pause',agent:'agt_00000000000000000000000001',idempotency_key:'pause-once'};
  emit({frame:'mutation',phase:'pending',id:'pause-once',command});
  emit({frame:'event',event:{seq:10,at:'2026-10-12T00:00:00Z',kind:{type:'custom',name:'submissions.evicted',payload:{n:1,oldest_at:null,max_submissions:100000}}}});
  assert.equal(store.get().outbox[0].status,'pending');assert.match(store.get().outbox[0].retentionRisk,/discarded/);
  assert.equal(store.get().activity.at(-1).kind,'submissions.evicted');store.dispose();
  const restored=createStore(mock().transport,{storage});
  assert.equal(restored.get().outbox[0].status,'unknown');assert.match(restored.get().outbox[0].retentionRisk,/current state/);restored.dispose();
});
test('restricted remote events retain cursor continuity without inventing admin details',()=>{
  const {transport,emit}=mock();const store=createStore(transport,{storage:null});
  try {
    emit({frame:'event',event:{seq:20,at:'2026-10-12T00:00:00Z',kind:{type:'custom',name:'remote.restricted',payload:{redacted:true}}}});
    assert.equal(store.get().lastSeq,20);assert.equal(store.get().activity.at(-1).kind,'remote.restricted');
    assert.match(store.get().activity.at(-1).detail,/only on the host/);
  } finally {store.dispose();}
});

test('uncertain delivery stays in its ledger instead of raising an unrelated global error',async()=>{
  const {transport}=mock();const store=createStore(transport,{storage:null});store.patch({connection:'online',role:'operator'});
  try {
    transport.request=async()=>{throw Object.assign(new Error('Receipt lost'),{outcome:'unknown'});};
    await assert.rejects(createCommand(store,transport)({cmd:'pause',agent:'agent'}),/Receipt lost/);
    assert.equal(store.get().error,null);
    transport.request=async()=>{throw Object.assign(new Error('Permission denied'),{code:'forbidden'});};
    await assert.rejects(createCommand(store,transport)({cmd:'pause',agent:'agent'}),/Permission denied/);
    assert.equal(store.get().error,null);
  }finally{store.dispose();}
});
