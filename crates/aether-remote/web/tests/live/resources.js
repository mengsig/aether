import {expect} from '@playwright/test';
import {until} from './host.js';
import {pairBrowser} from './browser.js';

/** @typedef {import('@playwright/test').Page} Page */
/** @typedef {import('./browser.js').Fixture} Fixture */

/** @param {Page} page @param {Fixture} fixture @param {string} id @param {string} panel */
async function openPanel(page,fixture,id,panel) {
  expect(id).toMatch(/^agt_[0-9A-Z]{26}$/);
  await page.goto(`${fixture.url}/#controls/${id}${panel ? `?panel=${panel}` : ''}`);
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible();
  await expect(page.locator('.controls-status')).not.toHaveText('Working…');
}

/** @param {Page} page @param {string} text */
async function confirmChange(page,text) {
  const dialog=page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText(text);
  await dialog.getByRole('button',{name:'Confirm',exact:true}).click();
}

/** @param {Page} page */
async function accepted(page) {
  await expect(page.locator('.controls-status')).toHaveText('Daemon accepted the change.');
  await expect(page.locator('.controls-error')).toHaveCount(0);
}

/** @param {Page} page @param {Fixture} fixture @param {string} id */
async function checkAgent(page,fixture,id) {
  await openPanel(page,fixture,id,'');
  const read=()=>fixture.request({cmd:'get_agent',id});
  await page.getByRole('button',{name:'Pause',exact:true}).click();
  await accepted(page);
  expect((await read()).data.paused).toBe(true);
  await page.getByRole('button',{name:'Resume',exact:true}).click();
  await accepted(page);
  expect((await read()).data.paused).toBe(false);
  const name='Release operations';
  await page.getByLabel('Name',{exact:true}).fill(name);
  await page.getByRole('button',{name:'Rename agent',exact:true}).click();
  await accepted(page);
  expect((await read()).data.name).toBe(name);
  await page.getByRole('combobox',{name:/^Reasoning effort/}).selectOption('high');
  await page.getByRole('button',{name:'Change effort',exact:true}).click();
  await accepted(page);
  expect((await read()).data.config.effort).toBe('high');
  await checkModel(page,fixture,id);
}

/** @param {Page} page @param {Fixture} fixture @param {string} id */
async function checkModel(page,fixture,id) {
  const catalogue=await fixture.request({cmd:'list_models'});
  expect(catalogue.reply).toBe('models');
  expect(catalogue.data.some((/** @type {{model:{provider:string,model:string}}} */ item)=>item.model.model==='mock-1')).toBe(true);
  const alternate=catalogue.data.some((/** @type {{model:{provider:string,model:string}}} */ item)=>item.model.model==='mock-2');
  await page.getByRole('button',{name:'Browse available models',exact:true}).click();
  for (const model of alternate?['mock-2','mock-1']:['mock-1']) {
    await page.getByRole('button',{name:`mock/${model}`,exact:true}).click();
    await expect(page.getByLabel('Provider/model',{exact:true})).toHaveValue(`mock/${model}`);
    await page.getByRole('button',{name:'Change model',exact:true}).click();
    await accepted(page);
    expect((await fixture.request({cmd:'get_agent',id})).data.model).toEqual({provider:'mock',model});
  }
}

/** @param {Page} page @param {Fixture} fixture @param {string} id */
async function checkSchedules(page,fixture,id) {
  await openPanel(page,fixture,id,'schedules');
  const name=`Live schedule ${Date.now()}`;
  await page.getByLabel('Schedule name',{exact:true}).fill(name);
  await page.getByLabel('Interval in seconds (interval only)',{exact:true}).fill('2');
  await page.getByLabel('Message for the agent',{exact:true}).fill('hello from the live schedule');
  await page.getByRole('button',{name:'Create schedule',exact:true}).click();
  await accepted(page);
  const reply=await fixture.request({cmd:'list_schedules',target:id});
  const schedule=reply.data.find((/** @type {{name:string}} */ item)=>item.name===name);
  expect(schedule?.id).toMatch(/^sch_/);
  expect(schedule.target).toBe(id);
  const row=page.locator('section.card').filter({has:page.getByRole('heading',{name,exact:true})});
  await row.getByRole('button',{name:'Pause',exact:true}).click();
  await accepted(page);
  expect((await fixture.request({cmd:'get_schedule',id:schedule.id})).data.status.state).toBe('paused');
  await row.getByRole('button',{name:'Resume',exact:true}).click();
  await accepted(page);
  const history=await until(()=>fixture.request({cmd:'schedule_history',id:schedule.id,limit:10}),value=>value.data.length>0,'schedule occurrence');
  expect(history.data[0].outcome.outcome).toBe('delivered');
  await row.getByRole('button',{name:'History',exact:true}).click();
  await expect(row.getByText('Delivered',{exact:true}).first()).toBeVisible();
  await row.getByRole('button',{name:'Cancel',exact:true}).click();
  await confirmChange(page,`Cancel schedule ${name}?`);
  await accepted(page);
  expect((await fixture.request({cmd:'get_schedule',id:schedule.id})).data.status.state).toBe('cancelled');
}

/** @param {Page} page @param {Fixture} fixture @param {string} id */
async function checkMemories(page,fixture,id) {
  await openPanel(page,fixture,id,'memories');
  const text=`Live memory ${Date.now()}: keep acceptance data isolated.`;
  await page.getByLabel('Memory',{exact:true}).fill(text);
  await page.getByRole('button',{name:'Create memory',exact:true}).click();
  await accepted(page);
  const query={scope:{scope:'agent',id},include_forgotten:true,limit:100};
  const read=async()=> (await fixture.request({cmd:'memories',query})).data.find((/** @type {{text:string}} */ item)=>item.text===text);
  const memory=await read();
  expect(memory?.id).toMatch(/^mem_/);
  expect(memory.pinned).toBe(false);
  const row=page.locator('section.card').filter({has:page.getByText(text,{exact:true})});
  await row.getByRole('button',{name:'Pin',exact:true}).click();
  await accepted(page);
  expect((await read()).pinned).toBe(true);
  await expect(row.getByText('Pinned for retrieval',{exact:true})).toBeVisible();
  await row.getByRole('button',{name:'Forget',exact:true}).click();
  await confirmChange(page,'Forget this memory?');
  await accepted(page);
  expect((await read()).forgotten_at).toBeTruthy();
  await expect(row).toHaveCount(0);
  await page.getByRole('combobox',{name:/^Forgotten memories/}).selectOption('Include');
  await page.getByRole('button',{name:'Search memories',exact:true}).click();
  await expect(row).toContainText('no longer retrieved');
}

/** @param {Page} page @param {Fixture} fixture @param {string} id */
async function checkArtifacts(page,fixture,id) {
  await fixture.request({cmd:'send_message',to:id,content:'big output',subtree:false});
  const reply=await until(()=>fixture.request({cmd:'list_artifacts',query:{agent:id,limit:100}}),value=>value.data.length>0,'large output artifact',30000);
  expect(reply.data[0].size_bytes).toBeGreaterThan(10000);
  expect(reply.data[0].agent_id).toBe(id);
  await openPanel(page,fixture,id,'artifacts');
  await page.getByRole('button',{name:'Preview file',exact:true}).first().click();
  const preview=page.locator('pre.controls-preview:visible');
  await expect(preview).toContainText('aether artifact line');
  await expect(page.getByRole('link',{name:'Download artifact',exact:true})).toHaveAttribute('href',/^blob:/);
}

/** @param {Page} page @param {Fixture} fixture @param {string} id */
async function checkReports(page,fixture,id) {
  await openPanel(page,fixture,id,'runs');
  await page.getByRole('button',{name:'Read run report',exact:true}).first().click();
  await expect(page.locator('.controls-report')).toBeVisible();
  await expect(page.locator('.controls-report')).toContainText('Input tokens');
  await openPanel(page,fixture,id,'diagnostics');
  await expect(page.getByRole('heading',{name:'Mock (local)',exact:true})).toBeVisible();
  await expect(page.getByText('No MCP servers configured.',{exact:true})).toBeVisible();
  await page.getByRole('button',{name:'Reconnect MCP servers',exact:true}).click();
  await confirmChange(page,'Reconnect all configured MCP servers');
  await accepted(page);
}

/** @param {Page} page @param {Fixture} fixture @param {string} id */
async function checkRules(page,fixture,id) {
  await openPanel(page,fixture,id,'rules');
  const scope=page.getByRole('combobox',{name:/^Scope/});
  expect(await scope.locator('option').evaluateAll(options=>options.map(option=>option.getAttribute('value')))).not.toContain('everywhere');
  const note=`Live rule ${Date.now()}`;
  await page.getByLabel('Tool matcher (blank matches every tool)',{exact:true}).fill('web_fetch');
  await page.getByLabel('Resource prefix (optional)',{exact:true}).fill('https://example.com/');
  await page.getByLabel('Note (optional)',{exact:true}).fill(note);
  await page.getByRole('button',{name:'Add approval rule',exact:true}).click();
  await confirmChange(page,'Add a denied rule for agent?');
  await accepted(page);
  const read=async()=>(await fixture.request({cmd:'list_approval_rules',include_revoked:true})).data.find((/** @type {{note:string}} */ item)=>item.note===note);
  const rule=await read();
  expect(rule.scope).toEqual({scope:'agent',id});
  expect(rule.decision).toBe('denied');
  const row=page.locator('section.card').filter({has:page.getByText(note,{exact:true})});
  await row.getByRole('button',{name:'Revoke rule',exact:true}).click();
  await confirmChange(page,`Revoke approval rule ${rule.id}?`);
  await accepted(page);
  expect((await read()).revoked).toBeTruthy();
}

/** @param {Page} page @param {Fixture} fixture @param {string} id */
async function checkResourcePermissions(page,fixture,id) {
  const browser=page.context().browser();
  if (!browser) throw new Error('Observer checks require a connected browser');
  expect(id).toMatch(/^agt_[0-9A-Z]{26}$/);
  const context=await browser.newContext({viewport:{width:390,height:844}});
  try {
    const observer=await context.newPage();
    const device=await pairBrowser(observer,fixture,'observer');
    await openPanel(observer,fixture,id,'');
    await expect(observer.getByRole('button',{name:'Pause',exact:true})).toBeDisabled();
    await expect(observer.getByRole('button',{name:'Change model',exact:true})).toBeDisabled();
    await openPanel(observer,fixture,id,'rules');
    await expect(observer.getByRole('button',{name:'Add approval rule',exact:true})).toBeDisabled();
    await expect(observer.getByRole('button',{name:'Refresh rules',exact:true})).toBeEnabled();
    await openPanel(observer,fixture,id,'memories');
    await expect(observer.getByRole('button',{name:'Create memory',exact:true})).toBeDisabled();
    await fixture.request({cmd:'revoke_device',id:device,reason:'Resource permissions checked'});
  } finally {await context.close();}
}

/** @param {Page} page @param {Fixture} fixture @param {string} agentId */
export async function checkResources(page,fixture,agentId) {
  expect(fixture.root).toMatch(/^\/var\/tmp\/aw-/);
  expect(agentId).toMatch(/^agt_[0-9A-Z]{26}$/);
  const seed=await fixture.request({cmd:'create_agent',name:'Live resource controls',role:'chat',objective:'hello',
    model:{provider:'mock',model:'mock-1'},scope:{workspace_root:fixture.workspace,network:'allowed'},
    config:{long_lived:true,effort:'low'},approval:{require_for:['external']}});
  expect(seed.reply).toBe('agent');
  const id=seed.data.id;
  await until(()=>fixture.request({cmd:'get_agent',id}),value=>value.data.status.state==='waiting','idle resource agent');
  await checkAgent(page,fixture,id);
  await checkSchedules(page,fixture,id);
  await checkMemories(page,fixture,id);
  await checkArtifacts(page,fixture,id);
  await checkReports(page,fixture,id);
  await checkRules(page,fixture,id);
  await checkResourcePermissions(page,fixture,id);
  return id;
}
