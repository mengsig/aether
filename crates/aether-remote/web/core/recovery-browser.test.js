import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { once } from 'node:events';
import { createMockServer } from '../dev/server.js';
const scratch=fileURLToPath(new URL('../node_modules/.recovery-browser/',import.meta.url));
process.env.TMPDIR=scratch;
process.env.PLAYWRIGHT_BROWSERS_PATH=fileURLToPath(new URL('../node_modules/.browsers/',import.meta.url));
await mkdir(scratch,{recursive:true});
const {chromium}=await import('@playwright/test');
const schema=JSON.parse(await readFile(new URL('../dev/protocol.schema.json',import.meta.url),'utf8'));
const supportsLookup=schema.$defs.Reply.oneOf.some(variant=>variant.properties?.reply?.const==='submission_resolved');
const enabled=(process.env.RUN_SUBMISSION_BROWSER ?? process.env.RUN_CONTROLS_BROWSER ?? '1')!=='0';
const original={cmd:'send_message',to:'agt_00000000000000000000000002',content:'Browser recovery exact payload',idempotency_key:'browser-key'};
async function harness(capability) {
  const server=createMockServer();server.submissions.enabled=capability;server.http.listen(0,'127.0.0.1');await once(server.http,'listening');
  const browser=await chromium.launch({headless:true});const page=await browser.newPage();const wire=[];
  page.on('websocket',socket=>socket.on('framesent',frame=>wire.push(JSON.parse(String(frame.payload)))));
  const base=`http://127.0.0.1:${server.http.address().port}`;
  return {server,browser,page,wire,base,close:async()=>{await browser.close();await server.close();}};
}
async function mount(env) {
  await env.page.goto(`${env.base}/api/health`);
  await env.page.evaluate(async()=>{
    const {createTransport}=await import('/core/transport.js');const {createStore}=await import('/core/store.js');
    const {loadIdentity}=await import('/core/identity.js');
    const transport=createTransport({timeout:3000,random:()=>1});const store=createStore(transport);
    window.recoveryTest={transport,store};
    if(await loadIdentity()) await transport.connect();else await transport.pair('aether-demo','Receipt recovery browser');
    await store.resolvePendingSubmissions();
  });
}
async function lose(env, command) {
  const error=await env.page.evaluate(async command=>{
    const {transport}=window.recoveryTest;
    try {await transport.request(command);return null;}
    catch(error){transport.disconnect();return error.message;}
  },command);
  assert.match(error,/Outcome unknown/);assert.ok(env.wire.some(row=>row.idempotency_key===command.idempotency_key));
}
async function reconnect(env) {
  await env.page.evaluate(async()=>{const {transport,store}=window.recoveryTest;await transport.connect();await store.resolvePendingSubmissions();});
}
async function state(env) {
  return env.page.evaluate(()=>({rows:window.recoveryTest.store.get().outbox,
    durable:Object.keys(localStorage).filter(key=>key.startsWith('aether.remote.receipt.v1:')).map(key=>JSON.parse(localStorage.getItem(key)))}));
}
test('browser reload preserves unknown exact payload without resolver capability', {skip:!enabled},async()=>{
  const env=await harness(false);try {
    await mount(env);const command={...original,mock_drop_reply:true};await lose(env,command);await mount(env);
    const current=await state(env);assert.equal(current.rows[0].recovery,'unsupported');assert.equal(current.durable[0].horizon,null);
    assert.deepEqual(current.durable[0].command,command);assert.equal(env.wire.filter(row=>row.cmd==='resolve_submission').length,0);
    assert.equal(env.server.state.transcripts.get(original.to).filter(row=>row.message.content[0].text===original.content).length,1);
  }finally{await env.close();}
});
test('browser receipt lookup after reload retires committed delivery without replay', {skip:!enabled||!supportsLookup},async()=>{
  const env=await harness(true);try {
    await mount(env);const command={...original,mock_drop_reply:true};await lose(env,command);const before=await state(env);await mount(env);
    const current=await state(env);assert.equal(current.durable.length,0);assert.equal(current.rows[0].status,'sent');
    const lookup=env.wire.find(row=>row.cmd==='resolve_submission');assert.deepEqual(lookup.command,command);assert.equal(lookup.horizon,before.durable[0].horizon);
    assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);assert.equal(current.rows[0].reply.reply,'message_sent');
  }finally{await env.close();}
});
test('browser absent_safe resends once after a lost lookup using its saved horizon', {skip:!enabled||!supportsLookup},async()=>{
  const env=await harness(true);try {
    await mount(env);env.server.submissions.dropNextMutation=true;await lose(env,original);
    const before=await state(env);env.server.submissions.dropNextLookup=true;await reconnect(env);await reconnect(env);
    const current=await state(env);assert.equal(current.durable.length,0);assert.equal(current.rows[0].status,'sent');
    assert.deepEqual(env.wire.filter(row=>row.cmd==='send_message').map(({req,...row})=>row),[original,original]);
    assert.ok(env.wire.filter(row=>row.cmd==='resolve_submission').every(row=>row.horizon===before.durable[0].horizon));
    assert.equal(env.server.state.transcripts.get(original.to).filter(row=>row.message.content[0].text===original.content).length,1);
  }finally{await env.close();}
});
test('browser forgotten and resolver errors remain unconfirmed without replay', {skip:!enabled||!supportsLookup},async()=>{
  const env=await harness(true);try {
    await mount(env);env.server.submissions.dropNextMutation=true;await lose(env,original);
    env.server.submissions.configure({cmd:'mock_submission_fault',forget:true});await reconnect(env);
    assert.equal((await state(env)).rows[0].recovery,'unknown');assert.match((await state(env)).rows[0].error,/no longer retained/);
    await env.page.evaluate(()=>window.recoveryTest.transport.disconnect());
    env.server.submissions.rejectLookup='not_found';await reconnect(env);
    assert.equal((await state(env)).rows[0].recovery,'error');assert.equal(env.wire.filter(row=>row.cmd==='send_message').length,1);
    assert.equal((await state(env)).durable.length,1);
  }finally{await env.close();}
});
