import {expect} from '@playwright/test';
import {pairBrowser} from './browser.js';

/** @typedef {import('./browser.js').Fixture} Fixture */
/** @param {import('@playwright/test').Browser} browser @param {Fixture} fixture */
export async function checkObserver(browser,fixture) {
  const context=await browser.newContext({viewport:{width:390,height:844},deviceScaleFactor:1});
  try {
    const page=await context.newPage();
    const id=await pairBrowser(page,fixture,'observer');
    await page.goto(`${fixture.url}/#new`);
    await expect(page.getByRole('heading',{name:'Observer access'})).toBeVisible();
    await expect(page.getByRole('button',{name:'Start agent'})).toHaveCount(0);
    await page.goto(`${fixture.url}/#settings`);
    await expect(page.getByText(/Observer access/).first()).toBeVisible();
    await fixture.request({cmd:'revoke_device',id,reason:'Acceptance complete'});
    await expect(page.locator('.connection')).toHaveAttribute('data-state','revoked',{timeout:15000});
    await expect(page.getByText(/device has been revoked/)).toBeVisible();
  } finally {await context.close();}
}

/** @param {import('@playwright/test').Page} page @param {Fixture} fixture @param {string} agent */
export async function checkRecovery(page,fixture,agent) {
  await page.goto(`${fixture.url}/#agent/${agent}`);
  const draft=page.getByRole('textbox',{name:'Message draft'});
  await draft.fill('Keep this draft through a daemon restart.');
  await fixture.restart();
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:25000});
  await expect(draft).toHaveValue('Keep this draft through a daemon restart.');
  await page.reload();
  await expect(draft).toHaveValue('Keep this draft through a daemon restart.');
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:15000});
  await page.evaluate(async()=>{await navigator.serviceWorker.ready;});
  await page.context().setOffline(true);await page.reload();
  await expect(page.locator('.connection[data-state="online"]:visible')).toHaveCount(0);
  await expect(page.locator('.toast:visible')).toHaveCount(0);
  await expect(draft).toHaveValue('Keep this draft through a daemon restart.');
  await fixture.request({cmd:'send_message',to:agent,content:'hello after offline acceptance'});
  await page.context().setOffline(false);
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:25000});
  await expect(page.locator('.conversation-message').filter({hasText:'hello after offline acceptance'})).toHaveCount(1,{timeout:15000});
}
