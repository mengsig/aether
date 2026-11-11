import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdir} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {createMockServer} from '../../dev/server.js';

const scratch = fileURLToPath(new URL('../../node_modules/.conversation-browser/', import.meta.url));
process.env.TMPDIR = scratch;
process.env.PLAYWRIGHT_BROWSERS_PATH = fileURLToPath(new URL('../../node_modules/.browsers/', import.meta.url));
await mkdir(scratch, {recursive:true});
const {chromium, expect} = await import('@playwright/test');
const id = '01M4PKMS9YTNDAYSSZ1Z0YEPG0';
const original = `[inbox 4] Message from user:local_socket#${id}: [authenticated operator instruction]\nhello after offline acceptance`;

async function mountHarness(page) {
  await page.evaluate(async ({original,id}) => {
    const {mount} = await import('/features/conversation/index.js');
    const agent = {id:`agt_${id}`,name:'Workspace lead',status:'waiting',role:'worker',model:'mock/mock-1'};
    const entry = (seq, role, content, minute = 0) => ({seq,at:`2026-10-12T04:${String(30 + minute).padStart(2,'0')}:00Z`,message:{role,content}});
    const entries = [entry(1,'assistant',[{type:'tool_result',content:[{type:'text',text:'Five entries found.'}]}]),
      entry(2,'assistant',[{type:'text',text:'Done. The workspace has 5 entries.'}]),
      entry(3,'user',[{type:'text',text:original}]),
      entry(4,'assistant',[{type:'text',text:'Hello. I am a mock model running inside Aether. Ask me to list files, write a file, or delegate.'}]),
      entry(5,'user',[{type:'text',text:`[inbox 5] Message from user:device:Phone(dev_${id})#${id}: [authenticated operator instruction]\nCount slowly`}],6),
      entry(6,'assistant',[{type:'text',text:'One. Two. Three.'}],6)];
    const state = {ready:true,connection:'online',connectionDetail:'',role:'operator',agents:[agent],transcripts:{[agent.id]:entries},streams:{},outbox:[]};
    const listeners = new Set(); let draft = 'Keep this draft through a daemon restart.';
    const store = {get:()=>state,subscribe:fn=>{listeners.add(fn);return()=>listeners.delete(fn);},
      getDraft:()=>draft,setDraft:(_id,text)=>{draft=text;},loadTranscript:async()=>{},
      patch:patch=>{Object.assign(state,patch);listeners.forEach(fn=>fn());}};
    const context = {store,transport:{request:async()=>({reply:'inbox',data:[]})},command:async()=>({reply:'ok'}),
      toast:()=>{},navigate:()=>{},confirm:async()=>false};
    const root = document.createElement('main'); document.body.replaceChildren(root);
    window.conversationTest = {state,store,entries,root,cleanup:mount(root,context,agent.id)};
  }, {original,id});
  await expect(page.locator('.conversation-notice')).toBeHidden();
  await expect.poll(()=>page.locator('.conversation-viewport').evaluate(node=>node.scrollHeight-node.clientHeight-node.scrollTop)).toBeLessThan(2);
}

async function verifyNote(page) {
  await page.evaluate(()=>{
    const {state,store,entries}=window.conversationTest; const agent=state.agents[0].id;
    const note={seq:7,at:'2026-10-12T04:37:00Z',message:{role:'note',content:[{type:'text',text:'3 items shipped · https://example.com/pull/12'}]}};
    store.patch({transcripts:{[agent]:[...entries,note]}});
  });
  const note = page.locator('.conversation-role-note');
  await expect(note).toHaveCount(1);
  await expect(note.locator('.conversation-source')).toHaveText('Note · woke nobody');
  await expect(note.locator('.conversation-content')).toContainText('3 items shipped');
  assert.equal(await page.locator('.conversation-role-user, .conversation-role-assistant').filter({hasText:'3 items shipped'}).count(),0);
}

async function verifyGeometry(page, name) {
  await page.evaluate(async()=>{await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);});
  await expect(page.getByRole('textbox', {name:'Message draft'})).toHaveValue('Keep this draft through a daemon restart.');
  await expect.poll(()=>page.locator('.conversation-viewport').evaluate(node=>node.scrollHeight-node.clientHeight-node.scrollTop)).toBeLessThan(2);
  await page.screenshot({path:`${scratch}/${name}.png`});
  const geometry = await page.evaluate(() => {
    const box = selector=>document.querySelector(selector).getBoundingClientRect();
    const header = box('.conversation-header'); const viewport = box('.conversation-viewport');
    const draft = box('.conversation-draft'); const composer = box('.conversation-composer');
    const command = box('.conversation-command-toggle'); const send = box('.conversation-send');
    const chips = [...document.querySelectorAll('.conversation-tool:not([open]) > summary')].map(node=>node.getBoundingClientRect());
    return {width:innerWidth,height:innerHeight,right:document.documentElement.scrollWidth,
      headerBottom:header.bottom,viewportTop:viewport.top,viewportBottom:viewport.bottom,composerTop:composer.top,
      composerBottom:composer.bottom,draftLeft:draft.left,draftBottom:draft.bottom,commandTop:command.top,sendBottom:send.bottom,
      textAlign:getComputedStyle(document.querySelector('.conversation-draft')).textAlign,
      clipped:chips.filter(box=>box.top<viewport.top && box.bottom>viewport.top).length};
  });
  assert.ok(geometry.right <= geometry.width, JSON.stringify(geometry));
  assert.ok(geometry.viewportTop >= geometry.headerBottom && geometry.viewportBottom <= geometry.composerTop, JSON.stringify(geometry));
  assert.ok(geometry.composerBottom <= geometry.height + 1 && geometry.sendBottom <= geometry.height, JSON.stringify(geometry));
  assert.equal(geometry.textAlign, 'left'); assert.ok(geometry.draftLeft < 30);
  assert.ok(geometry.draftBottom <= geometry.commandTop + 1); assert.equal(geometry.clipped, 0, JSON.stringify(geometry));
  await page.screenshot({path:`${scratch}/${name}.png`});
}

async function verifyActions(page) {
  const row = page.locator('[data-seq="3"]'); const content = row.locator('.conversation-content');
  await expect(content).toHaveText('From this computerhello after offline acceptance');
  const more = row.getByRole('button',{name:'Copy, quote, or select this message'});
  assert.equal(await more.evaluate(node=>getComputedStyle(node).opacity), '0');
  await content.click(); assert.equal(await more.evaluate(node=>getComputedStyle(node).opacity), '1');
  const contentBox = await content.boundingBox(); const moreBox = await more.boundingBox();
  assert.ok(contentBox && moreBox && moreBox.y >= contentBox.y + contentBox.height);
  await page.screenshot({path:`${scratch}/message-tap-actions.png`});
  await more.click(); await page.getByRole('button',{name:'Message details',exact:true}).click();
  await expect(row.locator('.conversation-provenance pre')).toHaveText(original);
  await row.locator('.conversation-provenance > summary').click();
  await expect(row.locator('.conversation-provenance')).toBeHidden();
  await more.focus(); await page.keyboard.press('Enter');
  await expect(page.getByRole('dialog',{name:'Message actions',exact:true})).toBeVisible();
  await page.screenshot({path:`${scratch}/message-actions.png`});
  await page.getByRole('button',{name:'Close',exact:true}).click();
  await content.dispatchEvent('pointerdown',{pointerType:'touch',clientX:50,clientY:100});
  await expect(page.getByRole('dialog',{name:'Message actions',exact:true})).toBeVisible();
  await content.dispatchEvent('pointerup',{pointerType:'touch'});
  await page.evaluate(()=>Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>{window.copiedMessage=text;}}}));
  await page.getByRole('button',{name:'Copy message',exact:true}).click();
  assert.equal(await page.evaluate(()=>window.copiedMessage),original);
  assert.equal(await page.locator('.conversation-time-group').count(), 2);
}

async function verifyLargeTextActions(page) {
  const more = page.locator('[data-seq="6"] .conversation-more');
  await more.focus(); await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog',{name:'Message actions',exact:true});
  await expect(dialog).toBeVisible();
  const box = await dialog.boundingBox();
  assert.ok(box && box.x >= 0 && box.x + box.width <= 360);
  assert.ok(box.y >= 0 && box.y + box.height <= 640);
  await page.screenshot({path:`${scratch}/actions-text200.png`});
  await page.getByRole('button',{name:'Close',exact:true}).click();
}

async function verifySearchAndConnection(page) {
  await page.keyboard.press('Control+f');
  await page.getByRole('searchbox',{name:'Search full conversation'}).fill(id);
  await expect(page.locator('.conversation-search-hit .conversation-provenance pre')).toHaveText(original);
  await page.keyboard.press('Escape');
  await page.evaluate(()=>window.conversationTest.store.patch({connection:'reconnecting'}));
  await expect(page.locator('.conversation-connection')).toHaveText('Reconnecting… · Your draft stays here');
  await expect(page.getByRole('button',{name:'Send',exact:true})).toBeDisabled();
  await expect(page.locator('.conversation-compose-hint')).toBeEmpty();
  await page.evaluate(()=>window.conversationTest.store.patch({connection:'offline'}));
  await expect(page.locator('.conversation-connection')).toHaveText('Offline · Your draft stays here');
  await page.getByRole('textbox',{name:'Message draft'}).fill('Draft edited while offline.');
  assert.equal(await page.evaluate(()=>window.conversationTest.store.getDraft()),'Draft edited while offline.');
  await page.screenshot({path:`${scratch}/offline.png`});
}

async function verifyToolEdge(page) {
  const height = await page.evaluate(()=> {
    const root = document.querySelector('.conversation-screen');
    const viewport = document.querySelector('.conversation-viewport');
    const chip = document.querySelector('.conversation-tool > summary');
    const oldHeight = root.style.getPropertyValue('--conversation-height');
    const style = getComputedStyle(viewport);
    const total = [...viewport.children].reduce((sum,node)=>sum+node.getBoundingClientRect().height,0) + parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
    const targetScroll = chip.getBoundingClientRect().top - viewport.getBoundingClientRect().top + chip.getBoundingClientRect().height/2;
    root.style.setProperty('--conversation-height', `${root.clientHeight - viewport.clientHeight + total - targetScroll}px`);
    return oldHeight;
  });
  await expect.poll(()=>page.locator('.conversation-viewport').evaluate(node=>Number(node.dataset.latestInset))).toBeGreaterThan(0);
  const clipped = await page.evaluate(()=> {
    const edge = document.querySelector('.conversation-viewport').getBoundingClientRect().top;
    const box = document.querySelector('.conversation-tool > summary').getBoundingClientRect();
    return box.top < edge && box.bottom > edge;
  });
  assert.equal(clipped,false);
  await page.screenshot({path:`${scratch}/tool-header-inset.png`});
  await page.evaluate(height=>document.querySelector('.conversation-screen').style.setProperty('--conversation-height',height),height);
}

async function verifyKeyboard(page) {
  await page.evaluate(()=> {
    const viewport = new EventTarget(); Object.assign(viewport,{height:390,offsetTop:0,scale:1});
    Object.defineProperty(window,'visualViewport',{configurable:true,value:viewport});
    window.conversationTest.cleanup();
  });
  await mountHarness(page); await page.getByRole('textbox',{name:'Message draft'}).focus();
  await expect(page.locator('.conversation-screen')).toHaveAttribute('data-keyboard','true');
  const box = await page.locator('.conversation-composer').boundingBox();
  assert.ok(box && box.y + box.height <= 391); assert.ok(box.y > 60);
  await page.screenshot({path:`${scratch}/keyboard-390.png`});
  await page.locator('[data-seq="6"] .conversation-more').focus(); await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog',{name:'Message actions',exact:true});
  await expect(dialog).toBeVisible();
  const actions = await dialog.boundingBox(); assert.ok(actions && actions.y >= 0 && actions.y + actions.height <= 390);
  await page.screenshot({path:`${scratch}/keyboard-actions.png`});
  await page.getByRole('button',{name:'Close',exact:true}).click();
}

async function verifyLongTranscript(page) {
  await page.evaluate(()=> {
    const {state,store,entries} = window.conversationTest;
    const rows = Array.from({length:5000},(_,index)=>({...entries[1],seq:100+index,
      message:{role:'assistant',content:[{type:'text',text:`Entry ${index}. ${'Preserve anchors and drafts. '.repeat(index%5+1)}`}]}}));
    store.patch({transcripts:{[state.agents[0].id]:rows}});
  });
  await expect(page.locator('[data-seq="5099"]')).toBeAttached();
  assert.ok(await page.locator('.conversation-message').count() < 40);
  await page.keyboard.press('Control+f');
  await page.getByRole('searchbox',{name:'Search full conversation'}).fill('Entry 2345.');
  await expect(page.locator('[data-seq="2445"]')).toBeVisible();
  const before = await page.locator('[data-seq="2445"]').boundingBox();
  await page.evaluate(()=>window.conversationTest.store.patch({streams:{latest:{agentId:window.conversationTest.state.agents[0].id,invocation:'latest',text:'New streamed answer',thinking:''}}}));
  await expect(page.locator('.conversation-stream-text:not(.muted)')).toHaveText('New streamed answer');
  const after = await page.locator('[data-seq="2445"]').boundingBox();
  assert.ok(before && after && Math.abs(after.y-before.y)<2);
  await page.getByRole('button',{name:'Jump to latest ↓'}).click();
  await expect.poll(()=>page.locator('.conversation-viewport').evaluate(node=>node.scrollHeight-node.clientHeight-node.scrollTop)).toBeLessThan(2);
}

test('conversation small screens, original records, actions, keyboard and virtual anchors', {
  skip:process.env.RUN_CONVERSATION_BROWSER !== '1',timeout:90000,
}, async () => {
  const server = createMockServer();
  await new Promise(resolve=>server.http.listen(0,'127.0.0.1',resolve));
  const address = server.http.address(); assert.ok(address && typeof address === 'object');
  let browser;
  try {
    browser = await chromium.launch({headless:true});
    const page = await browser.newPage({viewport:{width:390,height:844},hasTouch:true});
    page.setDefaultTimeout(10000); const errors=[]; page.on('pageerror',error=>errors.push(error.message));
    await page.route('**/boot.js',route=>route.fulfill({contentType:'text/javascript',body:''}));
    await page.goto(`http://127.0.0.1:${address.port}/`);
    await mountHarness(page); await verifyGeometry(page,'390x844'); await verifyToolEdge(page); await verifyActions(page);
    await verifySearchAndConnection(page); await verifyNote(page);
    await page.evaluate(()=>window.conversationTest.cleanup()); await mountHarness(page);
    await page.setViewportSize({width:360,height:640}); await verifyGeometry(page,'360x640');
    await page.evaluate(()=>{document.documentElement.style.fontSize='200%';});
    await verifyGeometry(page,'360x640-text200'); await verifyLargeTextActions(page);
    await page.evaluate(()=>{document.documentElement.style.fontSize='100%';});
    await page.setViewportSize({width:390,height:844}); await verifyKeyboard(page);
    await verifyLongTranscript(page); assert.deepEqual(errors,[]);
  } finally { await browser?.close(); await server.close(); }
});
