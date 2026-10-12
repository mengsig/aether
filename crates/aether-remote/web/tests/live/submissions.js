import {expect} from '@playwright/test';
import {until} from './host.js';

/** @param {import('@playwright/test').Page} page @param {string} content */
async function dropAcknowledgement(page,content) {
  const observed={key:'',writes:0,lookups:0,dropped:false,horizon:''};
  await page.routeWebSocket(/\/ws$/,socket=>{
    const server=socket.connectToServer();let request=0;
    socket.onMessage(message=>{
      const frame=JSON.parse(String(message));
      if (frame.cmd==='send_message' && frame.content===content) {
        observed.writes++;observed.key=frame.idempotency_key;request=frame.req;
      }
      if (frame.cmd==='resolve_submission' && frame.command.idempotency_key===observed.key) observed.lookups++;
      server.send(message);
    });
    server.onMessage(async message=>{
      const frame=JSON.parse(String(message));
      if (frame.result?.Ok?.reply==='welcome') observed.horizon=frame.result.Ok.data.submission_horizon || '';
      if (frame.frame==='reply' && frame.id===request && request && !observed.dropped) {
        expect(frame.result.Ok.reply).toBe('message_sent');observed.dropped=true;
        await socket.close({code:1012,reason:'Acceptance drops a committed acknowledgement'});
        await server.close({code:1012,reason:'Acceptance connection loss'});return;
      }
      socket.send(message);
    });
  });
  return observed;
}

/** @param {import('@playwright/test').Page} page @param {import('./browser.js').Fixture} fixture @param {string} agent */
export async function checkLostAcknowledgement(page,fixture,agent) {
  const content='hello after a lost acknowledgement';
  const observed=await dropAcknowledgement(page,content);
  await page.goto(`${fixture.url}/#agent/${agent}`);await page.reload();
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:15000});
  expect(observed.horizon,'The embedded daemon must advertise submission lookup').toBeTruthy();
  await page.getByRole('textbox',{name:'Message draft'}).fill(content);
  await page.getByRole('button',{name:'Send',exact:true}).click();
  await expect.poll(()=>observed.dropped).toBe(true);
  await expect.poll(()=>observed.lookups,{timeout:25000}).toBeGreaterThan(0);
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:25000});
  await expect(page.locator('.conversation-delivery-unknown,.conversation-delivery-pending')).toHaveCount(0,{timeout:15000});
  const inbox=await until(()=>fixture.request({cmd:'get_inbox',agent,after:0}),reply=>reply.data.some((/** @type {any} */ item)=>item.kind?.content?.some((/** @type {any} */ block)=>block.text===content)),'single delivered message');
  const delivered=inbox.data.filter((/** @type {any} */ item)=>item.kind?.content?.some((/** @type {any} */ block)=>block.text===content));
  expect(delivered).toHaveLength(1);expect(observed.writes).toBe(1);
  await expect(page.getByRole('button',{name:'Send again…'})).toHaveCount(0);
  await expect(page.locator('dialog[open]')).toHaveCount(0);
  console.log('Live submission recovery: committed acknowledgement lost, lookup recovered it, one original message and no operator prompt.');
}
