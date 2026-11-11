import test from 'node:test';
import assert from 'node:assert/strict';
import { readLedger, writeLedger, mergeLedger } from './ledger.js';
import { assertRetirementCapacity, readRetirement } from './retirements.js';
import { createStore } from './store.js';
function storage() {
  const values=new Map();return {get length(){return values.size;},key:index=>[...values.keys()][index]??null,
    getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value),removeItem:key=>values.delete(key)};
}
const original={id:'key',agentId:'agent',text:'exact',status:'unknown',command:{cmd:'send_message',to:'agent',content:'exact',idempotency_key:'key'},horizon:'first',attemptId:'record',sendId:'wire-a',createdOrder:1};
test('retirement dominates a stale write interleaved after the ledger read',()=>{
  const disk=storage();writeLedger(disk,'daemon','device',[original],[]);
  const set=disk.setItem;let interleaved=false;
  disk.setItem=(key,value)=>{
    if(!interleaved&&key.startsWith('aether.remote.receipt.')) {
      interleaved=true;writeLedger(disk,'daemon','device',[{...original,status:'sent'}],[original]);
      assert.equal(readLedger(disk,'daemon','device').length,0);
    }
    set(key,value);
  };
  const result=writeLedger(disk,'daemon','device',[{...original,recovery:'resolving'}],[original]);
  assert.equal(interleaved,true);assert.equal(readLedger(disk,'daemon','device').length,0);assert.equal(result[0].status,'sent');
});
test('old wire errors cannot erase or suppress the other tab current retry',()=>{
  const disk=storage();writeLedger(disk,'daemon','device',[original],[]);
  const retry={...original,status:'pending',sendId:'wire-b'};
  writeLedger(disk,'daemon','device',[retry],[original]);
  writeLedger(disk,'daemon','device',[{...original,status:'failed'}],[original]);
  assert.equal(readRetirement(disk,'daemon','device',original),'failed');assert.equal(readRetirement(disk,'daemon','device',retry),null);
  assert.equal(readLedger(disk,'daemon','device')[0].sendId,'wire-b');assert.equal(readLedger(disk,'daemon','device')[0].status,'pending');
  writeLedger(disk,'daemon','device',[{...original,recovery:'resolving'}],[original]);
  assert.equal(readLedger(disk,'daemon','device')[0].sendId,'wire-b');
});
test('interleaved old error removal cannot delete the replacement wire record',()=>{
  const disk=storage();writeLedger(disk,'daemon','device',[original],[]);
  const remove=disk.removeItem;let interleaved=false;const retry={...original,status:'pending',sendId:'wire-b'};
  disk.removeItem=key=>{
    if(!interleaved){interleaved=true;writeLedger(disk,'daemon','device',[retry],[{...original,status:'failed'}]);}
    remove(key);
  };
  writeLedger(disk,'daemon','device',[{...original,status:'failed'}],[original]);
  assert.equal(interleaved,true);assert.equal(readLedger(disk,'daemon','device')[0].sendId,'wire-b');
});
test('cross-tab terminal outcome preserves a local receipt after the pending key disappears',()=>{
  for(const status of ['sent','failed','dismissed']) {
    const disk=storage();writeLedger(disk,'daemon','device',[original],[]);
    writeLedger(disk,'daemon','device',[{...original,status}],[original]);
    const rows=mergeLedger(disk,'daemon','device',[original]);assert.equal(rows[0].status,status);assert.deepEqual(rows[0].command,original.command);
    assert.equal(readLedger(disk,'daemon','device').length,0);
  }
});
test('marker capacity stops new work visibly without purging retirement history',()=>{
  const disk={length:20_000,key:index=>`aether.remote.retired.v1:daemon:device:${index}:sent`};
  assert.throws(()=>assertRetirementCapacity(disk,'daemon','device',1),/capacity.*forget.*pair/);
  assert.doesNotThrow(()=>assertRetirementCapacity(disk,'other-daemon','device',1));
});
test('concurrent equal-order retries retire only the exact failed wire',()=>{
  const disk=storage();const first={...original,wireOrder:1};writeLedger(disk,'daemon','device',[first],[]);
  const set=disk.setItem;let interleaved=false;
  disk.setItem=(key,value)=>{
    if(!interleaved&&key.endsWith(':send-A')) {
      interleaved=true;
      const second=writeLedger(disk,'daemon','device',[{...first,status:'pending',sendId:'send-B'}],[first])[0];
      assert.equal(second.wireOrder,2);
      const settled=writeLedger(disk,'daemon','device',[{...second,status:'failed'}],[second])[0];
      assert.equal(settled.status,'unknown');assert.equal(settled.sendId,first.sendId);
    }
    set(key,value);
  };
  const pending=writeLedger(disk,'daemon','device',[{...first,status:'pending',sendId:'send-A'}],[first])[0];
  assert.equal(interleaved,true);assert.equal(pending.wireOrder,2);
  assert.equal(readLedger(disk,'daemon','device')[0].sendId,'send-A');
  const settled=writeLedger(disk,'daemon','device',[{...pending,status:'failed'}],[pending])[0];
  assert.equal(settled.status,'unknown');assert.equal(readLedger(disk,'daemon','device')[0].sendId,first.sendId);
  writeLedger(disk,'daemon','device',[{...first,status:'sent'}],[first]);
  assert.equal(readLedger(disk,'daemon','device').length,0);
});
test('both actual errors retire both wires even when the original tab missed the retry storage event',()=>{
  const disk=storage();const transport={onFrame:()=>()=>{},onConnection:()=>()=>{}};
  const first=createStore(transport,{storage:disk});const second=createStore(transport,{storage:disk});
  const session={frame:'session',daemonId:'daemon',deviceId:'device',role:'operator'};
  const receipt={frame:'mutation',command:original.command,id:original.id,horizon:'first'};
  try {
    first.receive(session);first.receive({...receipt,phase:'pending',sendId:'first'});
    first.receive({...receipt,phase:'unknown',sendId:'first'});
    second.receive(session);second.receive({...receipt,phase:'pending',sendId:'second'});
    first.receive({...receipt,phase:'failed',sendId:'first',late:true,error:'First rejected'});
    assert.equal(readLedger(disk,'daemon','device')[0].sendId,'second');
    second.receive({...receipt,phase:'failed',sendId:'second',error:'Second rejected'});
    assert.equal(readLedger(disk,'daemon','device').length,0);assert.equal(second.get().outbox[0].status,'failed');
  }finally{first.dispose();second.dispose();}
});
test('attempt capacity refuses before persistence and leaves all held wires recoverable',()=>{
  const disk=storage();let row=writeLedger(disk,'daemon','device',[original],[])[0];
  for(let index=1;index<256;index++) row=writeLedger(disk,'daemon','device',[{...row,status:'pending',sendId:`wire-${index}`}],[{...row,status:'unknown'}])[0];
  const count=disk.length;
  assert.throws(()=>writeLedger(disk,'daemon','device',[{...row,status:'pending',sendId:'over-limit'}],[{...row,status:'unknown'}]),/256 unconfirmed.*Reconcile/);
  assert.equal(disk.length,count);assert.equal(readLedger(disk,'daemon','device')[0].sendId,'wire-255');
  writeLedger(disk,'daemon','device',[{...row,status:'unknown',recovery:'unknown'}],[row]);
  assert.equal(readLedger(disk,'daemon','device')[0].recovery,'unknown');
  writeLedger(disk,'daemon','device',[{...row,status:'sent'}],[row]);assert.equal(readLedger(disk,'daemon','device').length,0);
});
test('storage bound measures UTF-8 bytes of every unresolved wire, not one logical command',()=>{
  const disk=storage();const content='😀'.repeat(120_000);
  const first={...original,text:'large',status:'pending',command:{...original.command,content}};
  const row=writeLedger(disk,'daemon','device',[first],[])[0];let latest=row;
  for(let index=1;index<4;index++) latest=writeLedger(disk,'daemon','device',[{...latest,status:'pending',sendId:`large-${index}`}],[{...latest,status:'unknown'}])[0];
  const count=disk.length;
  assert.throws(()=>writeLedger(disk,'daemon','device',[{...latest,status:'pending',sendId:'too-large'}],[{...latest,status:'unknown'}]),/storage is full.*Reconcile/);
  assert.equal(disk.length,count);assert.equal(readLedger(disk,'daemon','device').length,1);
  assert.equal(readLedger(disk,'daemon','device')[0].command.content,content);
});
test('concurrent tabs exceeding the soft attempt cap retain every unknown wire',()=>{
  const disk=storage();let row=writeLedger(disk,'daemon','device',[original],[])[0];
  for(let index=1;index<255;index++) row=writeLedger(disk,'daemon','device',[{...row,status:'pending',sendId:`wire-${index}`}],[{...row,status:'unknown'}])[0];
  const previous={...row,status:'unknown'};const set=disk.setItem;let interleaved=false;
  disk.setItem=(key,value)=>{
    if(!interleaved&&key.endsWith(':last-A')) {
      interleaved=true;writeLedger(disk,'daemon','device',[{...previous,status:'pending',sendId:'last-B'}],[previous]);
    }
    set(key,value);
  };
  writeLedger(disk,'daemon','device',[{...previous,status:'pending',sendId:'last-A'}],[previous]);
  assert.equal(interleaved,true);assert.equal(disk.length,257);
  const current=readLedger(disk,'daemon','device')[0];assert.ok(['last-A','last-B'].includes(current.sendId));
  writeLedger(disk,'daemon','device',[{...current,status:'unknown',recovery:'unknown'}],[current]);
  assert.equal(disk.length,257);
  assert.throws(()=>writeLedger(disk,'daemon','device',[{...current,status:'pending',sendId:'next'}],[{...current,status:'unknown'}]),/256 unconfirmed/);
  writeLedger(disk,'daemon','device',[{...current,status:'sent'}],[current]);assert.equal(readLedger(disk,'daemon','device').length,0);
});
test('new pending write is last even when another changed receipt retires in the same snapshot',()=>{
  const disk=storage();writeLedger(disk,'daemon','device',[original],[]);
  const fresh={...original,id:'new-key',sendId:'new-wire',status:'pending',command:{...original.command,idempotency_key:'new-key'}};
  const set=disk.setItem;disk.setItem=(key,value)=>{if(key.startsWith('aether.remote.retired.'))throw new Error('Marker quota');set(key,value);};
  assert.throws(()=>writeLedger(disk,'daemon','device',[fresh,{...original,status:'sent'}],[original]),/Marker quota/);
  assert.equal(readLedger(disk,'daemon','device').length,1);assert.equal(readLedger(disk,'daemon','device')[0].id,original.id);
});
test('no storage reads after first pending persistence can turn a committed row into failed preflight',()=>{
  const disk=storage();const settled={...original,status:'failed'};writeLedger(disk,'daemon','device',[settled],[]);
  const fresh={...original,id:'new-key',sendId:'new-wire',status:'pending',command:{...original.command,idempotency_key:'new-key'}};
  const set=disk.setItem;const get=disk.getItem;let committed=false;
  disk.setItem=(key,value)=>{set(key,value);if(key.endsWith(':new-wire'))committed=true;};
  disk.getItem=key=>{if(committed)throw new Error('Read denied after commit');return get(key);};
  const result=writeLedger(disk,'daemon','device',[fresh,settled],[settled]);
  assert.equal(committed,true);assert.equal(result[0].status,'pending');assert.equal(result[1].status,'failed');
  disk.getItem=get;assert.equal(readLedger(disk,'daemon','device')[0].id,'new-key');
});
test('a batch cannot partially persist multiple new sends',()=>{
  const disk=storage();const first={...original,status:'pending'};
  const second={...first,id:'second',sendId:'second-wire',command:{...first.command,idempotency_key:'second'}};
  assert.throws(()=>writeLedger(disk,'daemon','device',[first,second],[]),/one at a time/);
  assert.equal(disk.length,0);
});
