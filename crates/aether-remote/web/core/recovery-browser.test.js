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
  const browser=await chromium.launch({headless:true});const context=await browser.newContext();const page=await context.newPage();const wire=[];
  page.on('websocket',socket=>socket.on('framesent',frame=>wire.push(JSON.parse(String(frame.payload)))));
  const base=`http://127.0.0.1:${server.http.address().port}`;
  return {server,browser,page,wire,base,close:async()=>{await browser.close();await server.close();}};
}
async function mount(env,resolve = true,holdLookup = false) {
  await env.page.goto(`${env.base}/api/health`);
  await env.page.evaluate(async ({resolve,holdLookup})=>{
    const {createTransport}=await import('/core/transport.js');const {createStore}=await import('/core/store.js');
    const {loadIdentity}=await import('/core/identity.js');
    const transport=createTransport({timeout:3000,random:()=>1});const store=createStore(transport);
    window.recoveryTest={transport,store};
    if (holdLookup) {
      const resolveSubmission = transport.resolveSubmission.bind(transport);
      const gate = new Promise(resolve => { window.releaseLookup = resolve; });
      transport.resolveSubmission = async command => {
        const reply = await resolveSubmission(command); window.lookupHeld = true; await gate; return reply;
      };
    }
    if(await loadIdentity()) await transport.connect();else await transport.pair('aether-demo','Receipt recovery browser');
    if (resolve) await store.resolvePendingSubmissions();
  },{resolve,holdLookup});
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

test('two real pages exclude a competing unknown lookup after final preflight and before wire', {skip:!enabled}, async () => {
  const env = await harness(true); const second = await env.page.context().newPage();
  const secondWire = []; second.on('websocket', socket => socket.on('framesent', frame => secondWire.push(JSON.parse(String(frame.payload)))));
  try {
    await mount(env); env.server.submissions.dropNextMutation = true; await lose(env, original);
    const originalAttempt = (await state(env)).durable[0];
    await env.page.evaluate(() => {
      const {transport,store} = window.recoveryTest; const send = transport.send.bind(transport);
      const gate = new Promise(resolve => { window.releaseWire = resolve; });
      transport.send = async (command,sendId) => {
        if (command.cmd === 'send_message') { window.pausedSendId = sendId; window.beforeWire = true; await gate; }
        return send(command,sendId);
      };
      window.finishedRecovery = null;
      transport.connect().then(() => store.resolvePendingSubmissions()).then(
        () => { window.finishedRecovery = 'done'; }, error => { window.finishedRecovery = error.message; });
    });
    await env.page.waitForFunction(() => window.beforeWire === true);
    const before = await state(env);
    const current = before.rows.find(row => row.id === original.idempotency_key);
    const pending = before.durable.find(row => row.sendId === current.sendId);
    const previous = before.durable.find(row => row.sendId === originalAttempt.sendId);
    assert.equal(before.durable.length, 2); assert.notEqual(current.sendId, originalAttempt.sendId);
    assert.equal(current.sendId, await env.page.evaluate(() => window.pausedSendId));
    assert.equal(current.status, 'pending'); assert.equal(pending.status, 'pending'); assert.equal(previous.status, 'unknown');
    assert.deepEqual(pending.command, original); assert.deepEqual(previous.command, original);
    assert.equal(pending.horizon, originalAttempt.horizon); assert.equal(previous.horizon, originalAttempt.horizon);
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    env.server.submissions.configure({cmd:'mock_submission_fault',forget:true});
    await mount({...env,page:second}, false);
    await second.waitForFunction(() => window.recoveryTest.store.deferredRecovery.has('browser-key'));
    const exclusion = await second.evaluate(async () => ({locks:await navigator.locks.query(),
      markers:Object.keys(localStorage).filter(key => key.startsWith('aether.remote.unconfirmed.v1:'))}));
    assert.equal(exclusion.locks.held.length, 1); assert.equal(exclusion.locks.pending.length, 0);
    assert.deepEqual(exclusion.markers, []); assert.equal(secondWire.filter(row => row.cmd === 'resolve_submission').length, 0);
    await env.page.evaluate(() => window.releaseWire());
    await env.page.waitForFunction(() => window.finishedRecovery !== null);
    assert.equal(await env.page.evaluate(() => window.finishedRecovery), 'done');
    await second.evaluate(() => window.recoveryTest.store.resolvePendingSubmissions());
    assert.equal((await state(env)).durable.length, 0);
    assert.deepEqual(env.wire.filter(row => row.cmd === 'send_message').map(({req,...row}) => row), [original, original]);
    assert.equal(secondWire.filter(row => row.cmd === 'resolve_submission').length, 0);
    assert.equal(await second.evaluate(() => Object.keys(localStorage).some(key => key.startsWith('aether.remote.unconfirmed.v1:'))), false);
  } finally { await env.close(); }
});

test('two real pages reread permanent uncertainty written by the first lock owner', {skip:!enabled}, async () => {
  const env = await harness(true); const second = await env.page.context().newPage();
  try {
    await mount(env); env.server.submissions.dropNextMutation = true; await lose(env, original);
    env.server.submissions.sequence = 0; await mount({...env,page:second});
    assert.equal((await state({...env,page:second})).durable[0].unconfirmed.reason, 'horizon_ahead');
    env.server.submissions.sequence = 1; await reconnect(env);
    assert.equal((await state(env)).durable[0].unconfirmed.reason, 'horizon_ahead');
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    assert.equal((await state(env)).rows[0].status, 'unknown');
    await env.page.evaluate(() => window.recoveryTest.transport.request(window.recoveryTest.store.get().outbox[0].command));
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 2);
    assert.equal((await state(env)).durable.length, 0);
  } finally { await env.close(); }
});

test('browser without Web Locks latches absent_safe and sends only after explicit retry', {skip:!enabled}, async () => {
  const env = await harness(true);
  try {
    await env.page.addInitScript(() => Object.defineProperty(navigator, 'locks', {value:undefined}));
    await mount(env); env.server.submissions.dropNextMutation = true; await lose(env, original);
    await reconnect(env);
    const current = await state(env); assert.equal(current.durable[0].unconfirmed.reason, 'locks_unavailable');
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    await env.page.evaluate(() => window.recoveryTest.transport.request(window.recoveryTest.store.get().outbox[0].command));
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 2);
    assert.equal((await state(env)).durable.length, 0);
  } finally { await env.close(); }
});

test('two pages defer a held unknown reply then preserve its permanent uncertainty', {skip:!enabled}, async () => {
  const env = await harness(true);
  try {
    const second = await env.page.context().newPage(); const secondEnv = {...env,page:second};
    await mount(env); env.server.submissions.dropNextMutation = true; await lose(env, original);
    env.server.submissions.sequence = 0; await mount(secondEnv, false, true);
    await second.waitForFunction(() => window.lookupHeld === true);
    env.server.submissions.sequence = 1; await reconnect(env);
    assert.equal(await env.page.evaluate(() => window.recoveryTest.store.deferredRecovery.has('browser-key')), true);
    assert.equal(env.wire.filter(row => row.cmd === 'resolve_submission').length, 0);
    await env.page.evaluate(() => window.recoveryTest.store.resolvePendingSubmissions());
    assert.equal(env.wire.filter(row => row.cmd === 'resolve_submission').length, 0);
    assert.equal((await state(env)).durable[0].unconfirmed, undefined);
    await second.evaluate(() => window.releaseLookup());
    await second.evaluate(() => window.recoveryTest.store.resolvePendingSubmissions());
    await env.page.waitForFunction(() => window.recoveryTest.store.deferredRecovery.size === 0 && !window.recoveryTest.store.recovering);
    assert.equal((await state(env)).rows[0].unconfirmed.reason, 'horizon_ahead');
    assert.equal(env.wire.filter(row => row.cmd === 'resolve_submission').length, 1);
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
  } finally { await env.close(); }
});

test('an already connected second page recovers a foreign pending receipt after its lock holder closes', {skip:!enabled}, async () => {
  const env = await harness(true);
  try {
    const second = await env.page.context().newPage(); const secondWire = [];
    second.on('websocket', socket => socket.on('framesent', frame => secondWire.push(JSON.parse(String(frame.payload)))));
    await mount(env); await mount({...env,page:second});
    await env.page.evaluate(command => {
      const {transport} = window.recoveryTest; const send = transport.send.bind(transport);
      transport.send = async (input,sendId) => {
        if (input.cmd === 'send_message') { window.beforeWire = true; await new Promise(() => {}); }
        return send(input,sendId);
      };
      transport.request(command).catch(error => { window.requestError = error.message; });
    },original);
    await env.page.waitForFunction(() => window.beforeWire === true);
    await second.waitForFunction(() => window.recoveryTest.store.deferredRecovery.has('browser-key'));
    assert.equal(secondWire.filter(row => row.cmd === 'resolve_submission').length, 0);
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 0);
    const before = await state({...env,page:second}); assert.equal(before.durable[0].status, 'pending');
    await env.page.close();
    await second.waitForFunction(() => window.recoveryTest.store.get().outbox.find(row => row.id === 'browser-key')?.status === 'sent');
    const after = await state({...env,page:second}); assert.equal(after.durable.length, 0);
    assert.equal(secondWire.filter(row => row.cmd === 'hello').length, 1);
    assert.deepEqual(secondWire.filter(row => row.cmd === 'send_message').map(({req,...row}) => row), [original]);
    assert.equal(secondWire.filter(row => row.cmd === 'resolve_submission').length, 1);
    assert.equal(secondWire.find(row => row.cmd === 'resolve_submission').horizon, before.durable[0].horizon);
  } finally { await env.close(); }
});

test('a rejected browser lock API latches unavailable and never automatically resends', {skip:!enabled}, async () => {
  const env = await harness(true);
  try {
    await env.page.addInitScript(() => {
      window.lockRequests = 0;
      Object.defineProperty(navigator, 'locks', {value:{request:() => {
        window.lockRequests++; return Promise.reject(new DOMException('Lock access denied', 'SecurityError'));
      }}});
    });
    await mount(env); env.server.submissions.dropNextMutation = true; await lose(env, original); await reconnect(env);
    assert.equal((await state(env)).durable[0].unconfirmed.reason, 'locks_unavailable');
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 1);
    assert.equal(await env.page.evaluate(() => window.lockRequests), 1);
    await env.page.evaluate(() => window.recoveryTest.transport.request(window.recoveryTest.store.get().outbox[0].command));
    assert.equal(env.wire.filter(row => row.cmd === 'send_message').length, 2);
    assert.equal(await env.page.evaluate(() => window.lockRequests), 1);
    assert.equal((await state(env)).durable.length, 0);
  } finally { await env.close(); }
});
