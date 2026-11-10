import {expect} from '@playwright/test';
import {until} from './host.js';

/** @typedef {import('./browser.js').Fixture} Fixture */
/** @param {Fixture} fixture */
export async function seedChat(fixture) {
  const result=await fixture.request({cmd:'create_agent',role:'chat',name:'Workspace lead',scope:{workspace_root:fixture.workspace},config:{long_lived:true},model:{provider:'mock',model:'mock-1'}});
  expect(result.reply).toBe('agent');expect(result.data.id).toMatch(/^agt_[0-9A-Z]{26}$/);
  return /** @type {string} */ (result.data.id);
}

/** @param {import('@playwright/test').Page} page @param {Fixture} fixture @param {string} agent */
export async function checkChat(page,fixture,agent) {
  await page.goto(`${fixture.url}/#agent/${agent}`);
  const draft=page.getByRole('textbox',{name:'Message draft'});
  await draft.fill('Count slowly');
  await page.getByRole('button',{name:'Send',exact:true}).click();
  await expect(page.locator('.conversation-stream-text:not(.muted)')).toContainText('Counting',{timeout:10000});
  await expect(draft).toHaveValue('');
  await draft.fill('@REA');
  await expect(page.getByRole('button').filter({hasText:'README.md'})).toBeVisible();
  await page.getByRole('button').filter({hasText:'README.md'}).click();
  await expect(draft).toHaveValue(/README.md/);
  await draft.fill('/effort');
  await expect(page.getByRole('button').filter({hasText:'/effort'})).toBeVisible();
  await page.getByRole('button').filter({hasText:'/effort'}).click();
  await expect(page).toHaveURL(/#controls\//);
  await page.getByRole('button',{name:'Interrupt',exact:true}).click();
  await page.getByRole('button',{name:'Confirm',exact:true}).click();
  await until(()=>fixture.request({cmd:'get_agent',id:agent}),reply=>reply.data.status.state==='waiting','first response completed',30000);
  await page.goto(`${fixture.url}/#agent/${agent}`);
  await draft.fill('delegate deeply');
  await page.getByRole('button',{name:'Send',exact:true}).click();
  await until(()=>fixture.request({cmd:'list_agents',query:{}}),reply=>reply.data.some((/** @type {{parent?:string}} */ item)=>item.parent===agent),'real delegated child',30000);
  await draft.fill('');
}
