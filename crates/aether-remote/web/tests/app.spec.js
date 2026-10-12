import {test,expect} from '@playwright/test';
import {mkdir} from 'node:fs/promises';

/** @param {import('@playwright/test').Page} page @param {string} [secret] */
async function pair(page,secret='aether-demo') {
  await page.goto(`/#pair=${secret}`);
  await expect(page.getByRole('heading',{name:'Your agents. Within reach.'})).toBeVisible();
  await expect(page).toHaveURL(/#pair$/);
  await page.getByRole('button',{name:'Pair this device'}).click();
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:20000});
  await expect(page).toHaveURL(/#fleet$/);
}

/** @param {import('@playwright/test').Page} page */
async function captureFirstRun(page) {
  await mkdir('test-results/screenshots',{recursive:true});
  await page.goto('/#pair');
  await expect(page.getByText('Not paired',{exact:true})).toBeVisible();
  await expect(page.getByRole('navigation',{name:'Main navigation'})).toBeHidden();
  await expect(page.locator('.global-banner')).toBeHidden();
  for (const theme of ['light','dark']) {
    await page.evaluate(value => {document.documentElement.dataset.theme=value;document.documentElement.dataset.motion='reduced';},theme);
    await page.screenshot({path:`test-results/screenshots/pair-${theme}.png`,fullPage:true,animations:'disabled'});
    await page.screenshot({path:`test-results/screenshots/pair-${theme}-viewport.png`,animations:'disabled'});
  }
}

test('pairing, phone screens, themes and offline shell',async ({page,context}) => {
  test.setTimeout(90000);
  /** @type {string[]} */ const errors = [];
  page.on('pageerror',error => errors.push(error.message));
  await captureFirstRun(page);
  await pair(page);
  await expect(page.locator('.toast')).toBeHidden({timeout:10000});
  await mkdir('test-results/screenshots',{recursive:true});
  await page.getByRole('button',{name:'List',exact:true}).click();
  const first = page.locator('a[href^="#agent/"]').first();
  const agentRoute = (await first.getAttribute('href'))?.slice(1);
  expect(agentRoute).toBeTruthy();
  const routes = ['fleet','attention','activity','settings','new',agentRoute,agentRoute?.replace('agent/','controls/')];
  for (const theme of ['light','dark']) {
    await page.evaluate(value => { localStorage.setItem('aether.preferences',JSON.stringify({theme:value,motion:'reduced'}));document.documentElement.dataset.theme=value; },theme);
    await page.reload();
    for (const route of routes) {
      await page.goto(`/#${route}`);
      await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible();
      await page.waitForTimeout(250);
      await expect(page.locator('main h1')).toBeVisible();
      if (route?.startsWith('agent/')) {
        await expect(page.locator('.conversation-message').first()).toBeAttached();
        await expect.poll(() => page.locator('.conversation-viewport').evaluate(node => node.scrollHeight-node.clientHeight-node.scrollTop)).toBeLessThan(2);
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBeTruthy();
      await page.screenshot({path:`test-results/screenshots/${route?.split('/')[0]}-${theme}.png`,fullPage:true});
      await page.screenshot({path:`test-results/screenshots/${route?.split('/')[0]}-${theme}-viewport.png`});
    }
  }
  await page.goto('/#fleet');
  await page.evaluate(async () => { await navigator.serviceWorker.ready; });
  await context.setOffline(true); await page.reload();
  await expect(page.getByRole('heading',{name:'Your fleet',exact:true})).toBeVisible();
  await expect(page.locator('.connection[data-state="online"]:visible')).toHaveCount(0);
  await expect(page.locator('.toast:visible')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('conversation sends safely, keeps drafts and recovers the socket',async ({page,context}) => {
  await pair(page);
  await page.goto('/#agent/agt_00000000000000000000000001');
  await page.getByRole('button',{name:'Conversation options'}).click();
  await page.getByText('Inbox deliveries',{exact:true}).click();
  await expect(page.getByText('Decided by: Operator Alex · Review phone',{exact:true})).toBeVisible();
  await expect(page.locator('.toast')).toBeHidden({timeout:10000});
  for (const theme of ['light','dark']) {
    await page.evaluate(value=>{document.documentElement.dataset.theme=value;},theme);
    await page.screenshot({path:`test-results/screenshots/agent-inbox-${theme}.png`,fullPage:true});
  }
  await page.evaluate(()=>{document.documentElement.dataset.theme='light';});
  await page.getByRole('button',{name:'Close conversation options'}).click();
  const input = page.getByRole('textbox',{name:'Message draft'});
  await input.fill('Review keyboard access\nand preserve all history.');
  await page.reload();
  await expect(input).toHaveValue('Review keyboard access\nand preserve all history.');
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible();
  await page.getByRole('button',{name:'Send',exact:true}).click();
  await expect(input).toHaveValue('');
  await expect(page.locator('.conversation-delivery-sent')).toBeAttached();
  await expect(page.getByText('Recent deliveries',{exact:true})).toBeVisible();
  await expect(page.locator('.conversation-stream-text:not(.muted)')).toContainText('Received');
  await input.fill('Draft while offline');
  await context.setOffline(true);
  await page.reload();
  await expect(page.locator('.connection[data-state="online"]:visible')).toHaveCount(0);
  await expect(page.locator('.toast:visible')).toHaveCount(0);
  await context.setOffline(false); await page.reload();
  await expect(input).toHaveValue('Draft while offline');
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible();
});

test('operator resources render, lifecycle changes and exact decisions resolve',async ({page}) => {
  await pair(page);
  await page.goto('/#controls/agt_00000000000000000000000001');
  await page.reload();
  await expect(page.getByRole('button',{name:'Pause',exact:true})).toBeEnabled();
  await page.getByRole('button',{name:'Pause',exact:true}).click();
  await expect(page.locator('.controls-screen')).toContainText('Pause requested · the current turn may still be finishing.');
  await page.getByRole('button',{name:'Resume',exact:true}).click();
  for (const panel of ['schedules','memories','artifacts','runs','diagnostics','rules']) {
    await page.goto(`/#controls/agt_00000000000000000000000001?panel=${panel}`);
    await expect(page.locator('main h1')).toBeVisible();
    await page.waitForTimeout(150);
    await expect(page.locator('.controls-error')).toHaveCount(0);
    for (const theme of ['light','dark']) {
      await page.evaluate(value => {document.documentElement.dataset.theme=value;document.documentElement.dataset.motion='reduced';},theme);
      await page.screenshot({path:`test-results/screenshots/controls-${panel}-${theme}.png`,fullPage:true,animations:'disabled'});
    }
  }
  await page.goto('/#attention?item=apr_00000000000000000000000001');
  await page.getByRole('button',{name:'Approve…',exact:true}).click();
  await expect(page.getByRole('dialog')).toContainText('git push origin remote-dashboard');
  await page.getByRole('button',{name:'Keep as is'}).click();
  await expect(page.getByRole('button',{name:'Approve…',exact:true})).toBeVisible();
  await page.getByRole('button',{name:'Deny…',exact:true}).click();
  await page.getByRole('button',{name:'Confirm',exact:true}).click();
  await expect(page.getByRole('button',{name:'Approve…',exact:true})).toHaveCount(0);
  await page.goto('/#attention?item=question-6');
  await expect(page.locator('[data-attention="question-6"]')).toBeFocused();
  await page.getByLabel('Your reply').fill('Yes, keep archived work available in read-only history.');
  await page.getByRole('button',{name:'Send reply',exact:true}).click();
  await expect(page.getByLabel('Your reply')).toHaveCount(0);
});

test('observer can inspect the fleet and cannot start work',async ({page}) => {
  await pair(page,'aether-observer');
  await expect(page.getByRole('button',{name:'Start new work'})).toHaveCount(0);
  await page.goto('/#new');
  await expect(page.getByRole('heading',{name:'Observer access'})).toBeVisible();
  await expect(page.getByRole('button',{name:'Start agent'})).toHaveCount(0);
  await page.goto('/#attention');
  await expect(page.getByRole('button',{name:/Approve|Deny|Send answer/})).toHaveCount(0);
});

test('outbox command opens delivery review with keyboard focus',async({page})=>{
  await pair(page);
  await page.goto('/#agent/agt_00000000000000000000000001');
  await page.getByRole('button',{name:'/ Commands',exact:true}).click();
  await page.getByRole('searchbox',{name:'Filter commands'}).fill('outbox');
  await page.getByRole('button',{name:'/outbox Review unconfirmed deliveries',exact:true}).click();
  await expect(page).toHaveURL(/#settings\?panel=outbox$/);
  const heading=page.getByRole('heading',{name:'Delivery status',exact:true});
  await expect(heading).toBeInViewport({ratio:1});
  await expect(page.getByRole('region',{name:'Delivery status'})).toBeFocused();
});

test('workspace, model and objective create a root agent',async ({page}) => {
  await pair(page);
  await page.getByRole('button',{name:'Start new work'}).click();
  await page.getByLabel('Objective',{exact:true}).fill('Review the mobile authentication flow');
  await page.getByText('Options',{exact:true}).click();
  await page.getByLabel('Name',{exact:true}).fill('Mobile review');
  await page.locator('.workspace-choice').first().click();
  await expect(page.getByText(/Working tree clean|Uncommitted changes/)).toBeVisible();
  await page.getByRole('button',{name:'Start agent',exact:true}).click();
  await expect(page).toHaveURL(/#agent\//);
  await expect(page.getByRole('heading',{name:'Mobile review',exact:true})).toBeVisible();
});

test('tablet and desktop preserve navigation, reading width and large text',async ({page}) => {
  await pair(page);
  for (const width of [768,1280]) {
    await page.setViewportSize({width,height:900});
    await page.goto('/#fleet');
    await expect(page.getByRole('heading',{name:'Your fleet'})).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBeTruthy();
    await page.screenshot({path:`test-results/screenshots/fleet-${width}.png`});
  }
  await page.setViewportSize({width:320,height:844});
  await page.evaluate(() => {document.documentElement.style.fontSize='200%';});
  await expect(page.getByRole('heading',{name:'Your fleet'})).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBeTruthy();
  expect(await page.locator('.connection').evaluate(node => node.getBoundingClientRect().top)).toBeGreaterThanOrEqual(0);
  await page.screenshot({path:'test-results/screenshots/fleet-large-text.png'});
});

test('operator schedules bounded fresh-agent runs after reviewing confirmation',async ({page}) => {
  await pair(page);
  await page.goto('/#controls/agt_00000000000000000000000001?panel=schedules');
  await page.getByText('Schedule new runs',{exact:true}).click();
  await page.getByLabel('New-run schedule name',{exact:true}).fill('Morning workspace review');
  await page.getByLabel('Objective for each run',{exact:true}).fill('Review the workspace and report changes');
  await page.getByLabel('Host workspace',{exact:true}).fill('/work/aether');
  await page.getByLabel('Provider/model',{exact:true}).fill('codex/gpt-6-astra');
  await page.getByLabel('Maximum runs',{exact:true}).fill('3');
  await page.getByRole('button',{name:'Create scheduled run',exact:true}).click();
  await expect(page.getByRole('dialog')).toContainText('for up to 3 runs');
  await page.getByRole('button',{name:'Confirm',exact:true}).click();
  await expect(page.getByRole('heading',{name:'Morning workspace review',exact:true})).toBeVisible();
  await expect(page.locator('.controls-error')).toHaveCount(0);
});

test('tasks use an existing owner and compact preserves the selected agent',async ({page}) => {
  await pair(page);
  await page.goto('/#new');
  await page.getByText('Options',{exact:true}).click();
  await page.getByRole('radio',{name:'Existing agent task',exact:true}).check();
  await page.getByLabel('Objective',{exact:true}).fill('Audit notification lifecycle recovery');
  await page.getByRole('radio',{name:'Release conductor',exact:true}).check();
  await page.getByRole('button',{name:'Create task',exact:true}).click();
  await expect(page).toHaveURL(/#fleet$/);
  await expect(page.locator('.toast')).toContainText('Task created.');
  await page.goto('/#controls/agt_00000000000000000000000001');
  await page.getByRole('button',{name:'Compact',exact:true}).click();
  await expect(page.locator('.controls-status')).toHaveText('Daemon accepted the change.');
  await expect(page.locator('.controls-error')).toHaveCount(0);
});

test('system appearance updates browser chrome while a manual theme stays selected',async ({page}) => {
  await pair(page);
  await page.emulateMedia({colorScheme:'dark'});
  await expect(page.locator('meta[name="theme-color"]')).toHaveAttribute('content','#0d1216');
  await page.goto('/#settings');
  await page.getByRole('combobox',{name:'Theme',exact:true}).selectOption('light');
  await page.getByRole('button',{name:'Save appearance',exact:true}).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme','light');
  await page.emulateMedia({colorScheme:'light'}); await page.emulateMedia({colorScheme:'dark'});
  await expect(page.locator('meta[name="theme-color"]')).toHaveAttribute('content','#f6f7f9');
});
