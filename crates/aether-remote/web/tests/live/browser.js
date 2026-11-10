import {expect} from '@playwright/test';
import {mkdir} from 'node:fs/promises';
import {until} from './host.js';

/** @typedef {Awaited<ReturnType<import('./fixture.js').liveFixture>>} Fixture */
/** @param {import('@playwright/test').Page} page @param {Fixture} fixture @param {'operator'|'observer'} role */
export async function pairBrowser(page,fixture,role) {
  const pairing = (await fixture.request({cmd:'start_pairing',role})).data;
  expect(pairing.code).toBeTruthy(); expect(pairing.state).toBe('open');
  await page.goto(`${fixture.url}/#pair=${pairing.code}`);
  await page.getByRole('textbox',{name:'Device name'}).fill(`Live ${role} phone`);
  if (role==='operator') await capturePair(page);
  await page.getByRole('button',{name:'Pair this device'}).click();
  const claimed = await until(()=>fixture.request({cmd:'get_pairing',id:pairing.id}),reply=>reply.data.state==='claimed','browser claim');
  await expect(page.getByText(claimed.data.candidate.phrase,{exact:true})).toBeVisible();
  const enrolled = await fixture.request({cmd:'confirm_pairing',id:pairing.id,accept:true});
  expect(enrolled.data.state).toBe('enrolled');
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:20000});
  return enrolled.data.device.id;
}

/** @param {import('@playwright/test').Page} page */
async function capturePair(page) {
  await mkdir('test-results/live/screenshots',{recursive:true});
  for (const theme of ['light','dark']) {
    await settleTheme(page,theme);
    await page.screenshot({path:`test-results/live/screenshots/pair-${theme}.png`,animations:'disabled'});
  }
  await page.evaluate(()=>{document.documentElement.dataset.theme='light';});
}

/** @param {import('@playwright/test').Page} page @param {string} theme */
async function settleTheme(page,theme) {
  expect(['light','dark']).toContain(theme);
  await page.evaluate(value=>{
    document.documentElement.dataset.motion='reduced';
    document.documentElement.dataset.theme=value;
  },theme);
  await expect(page.locator('body')).toHaveCSS('background-color',theme==='light'?'rgb(246, 247, 249)':'rgb(13, 18, 22)');
  await expect(page.locator('main')).toHaveCSS('opacity','1');
  await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve(null)))));
}

/** @param {Fixture} fixture */
export async function seedCaptureAttention(fixture) {
  const base={cmd:'create_agent',role:'worker',scope:{workspace_root:fixture.workspace,network:'allowed'},model:{provider:'mock',model:'mock-1'},approval:{require_for:['external']}};
  await fixture.request({...base,name:'Deployment review',objective:'Fetch the changelog page'});
  await fixture.request({...base,name:'Release decision',objective:'live acceptance question'});
  await until(()=>fixture.request({cmd:'get_attention'}),reply=>reply.data.approvals.length>0 && reply.data.questions.length>0,'pending decisions for capture');
}

/** @param {import('@playwright/test').Page} page @param {Fixture} fixture */
export async function startAgent(page,fixture) {
  await page.goto(`${fixture.url}/#new`);
  await page.getByLabel('Objective',{exact:true}).fill('hello');
  await page.getByText('Options',{exact:true}).click();
  await page.getByLabel('Name',{exact:true}).fill('Mobile acceptance');
  await page.getByRole('button',{name:'Browse',exact:true}).click();
  await page.getByLabel('Host workspace',{exact:true}).fill(`${fixture.root}/work`);
  await page.getByRole('button',{name:new RegExp(`^Use ${fixture.workspace}/?$`)}).click();
  await expect(page.getByText(/Working tree clean/)).toBeVisible();
  await expect(page.getByRole('radio',{name:'Mock model',exact:true})).toBeChecked();
  await page.getByRole('button',{name:'Start agent',exact:true}).click();
  await expect(page).toHaveURL(/#agent\/agt_/,{timeout:15000});
  const id = new URL(page.url()).hash.slice('#agent/'.length);
  expect(id).toMatch(/^agt_[0-9A-Z]{26}$/);
  await expect(page.locator('.conversation-message').filter({hasText:'Hello. I am a mock model'})).toBeVisible({timeout:20000});
  return id;
}

/** @param {import('@playwright/test').Page} page @param {Fixture} fixture @param {string} id @param {string} resourceId */
export async function captureScreens(page,fixture,id,resourceId) {
  await mkdir('test-results/live/screenshots',{recursive:true});
  await expect(page.locator('.toast')).toBeHidden({timeout:10000});
  const routes=['fleet','attention','activity','new','settings',`agent/${id}`,`controls/${resourceId}`];
  for (const panel of ['schedules','memories','artifacts','runs','diagnostics','rules']) routes.push(`controls/${resourceId}?panel=${panel}`);
  for (const theme of ['light','dark']) {
    await settleTheme(page,theme);
    for (const route of routes) {
      await page.goto(`${fixture.url}/#${route}`);
      await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible();
      await expect(page.locator('main h1')).toBeVisible();
      await page.waitForTimeout(400);
      expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
      await checkNavigationClearance(page);
      const name=route.includes('?')?`controls-${route.split('=')[1]}`:route.split('/')[0];
      await page.screenshot({path:`test-results/live/screenshots/${name}-${theme}.png`,animations:'disabled'});
      const errors=await page.locator('.controls-error,.error-banner:visible').allTextContents();
      expect(errors.filter(Boolean),`No visible errors on ${route}`).toEqual([]);
      console.log(route,errors.filter(Boolean));
    }
  }
}

/** @param {import('@playwright/test').Page} page */
async function checkNavigationClearance(page) {
  const navigation=page.getByRole('navigation',{name:'Main navigation'});
  if (!await navigation.isVisible()) return;
  const content=await page.locator('main').boundingBox();
  const bar=await navigation.boundingBox();
  expect(content && bar,'Both scroll content and navigation must have geometry').toBeTruthy();
  expect((content?.y ?? 0)+(content?.height ?? 0),'Content must end above navigation').toBeLessThan(bar?.y ?? 0);
}

/** @param {import('@playwright/test').Page} page @param {Fixture} fixture @param {number} count */
export async function captureFleetSize(page,fixture,count) {
  expect([1,10,60]).toContain(count);
  const agents=(await fixture.request({cmd:'list_agents',query:{}})).data;
  expect(agents.length).toBeLessThanOrEqual(count);
  const names=['Interface','Transport','Security','Review','Testing','Docs'];
  for (let index=agents.length;index<count;index++) {
    const created=await fixture.request({cmd:'create_agent',name:`${names[index%names.length]} ${Math.floor(index/6)+1}`,role:'worker',
      ...(index%6 ? {parent:agents[Math.max(0,index-index%6)].id} : {}),scope:{workspace_root:fixture.workspace},model:{provider:'mock',model:'mock-1'},config:{effort:'medium'}});
    expect(created.reply).toBe('agent');agents.push(created.data);
  }
  await page.goto(`${fixture.url}/#fleet`);
  await expect(page.locator('.fleet-graph-node')).toHaveCount(count);
  await expect(page.locator('.toast')).toBeHidden({timeout:10000});
  await mkdir('test-results/live/screenshots',{recursive:true});
  for (const theme of ['light','dark']) {
    await settleTheme(page,theme);
    await page.screenshot({path:`test-results/live/screenshots/fleet-${count}-${theme}.png`,animations:'disabled'});
  }
  await page.evaluate(()=>{document.documentElement.dataset.theme='light';});
}
