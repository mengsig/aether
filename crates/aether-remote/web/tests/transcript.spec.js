import {test,expect} from '@playwright/test';
import {createMockServer} from '../dev/server.js';
import {writeFile} from 'node:fs/promises';

function longTranscriptServer() {
  const server = createMockServer();
  const agent = server.state.agents[0].id;
  const template = server.state.transcripts.get(agent)?.[0];
  if (!template) throw new Error('Transcript fixture is required');
  const entries = Array.from({length:5000},(_,index) => ({...template,seq:100+index,
    message:{role:'assistant',content:[{type:'text',text:`## Entry ${index}\n\n${'Keep the current task clear and preserve operator decisions. '.repeat(index%6+1)}\n\n- Check the contract\n- Preserve the result`}]} }));
  server.state.transcripts.set(agent,entries); server.state.seq = 5100;
  return {server,agent};
}

/** @param {import('@playwright/test').Locator} viewport */
async function sampleScrolling(viewport) {
  return viewport.evaluate(async node => {
    node.scrollTop = 0;
    await new Promise(resolve => requestAnimationFrame(resolve));
    const intervals = [];
    let previous = performance.now(); let peakMounted = 0;
    for (let index = 0; index < 120; index++) {
      node.scrollTop += 180;
      const now = await new Promise(resolve => requestAnimationFrame(resolve));
      if (index > 5) intervals.push(now-previous);
      previous = now;
      peakMounted = Math.max(peakMounted,node.querySelectorAll('.conversation-message').length);
    }
    intervals.sort((left,right) => left-right);
    return {frames:intervals.length,peakMounted,p95FrameMs:Number(intervals[Math.floor(intervals.length*.95)].toFixed(2)),
      framesAbove34ms:intervals.filter(interval => interval > 34).length};
  });
}

test('five thousand messages stay bounded, searchable and anchored during streaming',async ({page}) => {
  const {server,agent} = longTranscriptServer();
  await new Promise(resolve => server.http.listen(0,'127.0.0.1',() => resolve(undefined)));
  const address = server.http.address();
  if (!address || typeof address === 'string') throw new Error('Expected a loopback mock listener');
  try {
    await page.goto(`http://127.0.0.1:${address.port}/#code=aether-demo`);
    await page.getByRole('button',{name:'Pair this device'}).click(); await expect(page).toHaveURL(/#fleet$/);
    await page.goto(`http://127.0.0.1:${address.port}/#agent/${agent}`);
    const viewport = page.locator('.conversation-viewport');
    await expect(page.locator('[data-seq="5099"]')).toBeAttached();
    await expect.poll(() => viewport.evaluate(node => node.scrollHeight-node.clientHeight-node.scrollTop)).toBeLessThan(2);
    const mounted = await page.locator('.conversation-message').count(); expect(mounted).toBeLessThan(40);
    const scrolling = await sampleScrolling(viewport); expect(scrolling.peakMounted).toBeLessThan(40);
    await page.keyboard.press('Control+f');
    await page.getByRole('searchbox',{name:'Search full conversation'}).fill('Entry 2345');
    await expect(page.locator('.conversation-search-hit')).toContainText('Entry 2345');
    const anchor = await page.locator('[data-seq="2445"]').elementHandle(); expect(anchor).not.toBeNull();
    const before = await viewport.evaluate(node => node.scrollTop);
    server.broadcastMessage({cmd:'send_message',to:agent,content:'A concurrent operator update'},'msg_00000000000000000000000099');
    await expect(page.locator('.conversation-stream-text:not(.muted)')).toContainText('Received');
    expect(await anchor?.evaluate(node => node.isConnected)).toBe(true);
    expect(Math.abs(await viewport.evaluate(node => node.scrollTop)-before)).toBeLessThan(2);
    await page.getByRole('button',{name:'Jump to latest ↓'}).click();
    await expect.poll(() => viewport.evaluate(node => node.scrollHeight-node.clientHeight-node.scrollTop)).toBeLessThan(2);
    expect(await page.locator('.conversation-message').count()).toBeLessThan(40);
    await page.screenshot({path:'test-results/screenshots/conversation-5000.png',animations:'disabled'});
    await writeFile('test-results/transcript-performance.json',JSON.stringify({entries:5000,mounted,scrolling,streamAnchorPreserved:true},null,2));
  } finally { await server.close(); }
});
