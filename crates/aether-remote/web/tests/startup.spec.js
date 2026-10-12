import {test,expect} from '@playwright/test';

test('an interrupted first load offers reload without exposing raw errors',async({page})=>{
  await page.route('**/features/fleet/graph/nodes.js',route=>route.abort('internetdisconnected'));
  await page.goto('/#pair=aether-demo');
  await expect(page.getByRole('heading',{name:'Couldn’t open Aether'})).toBeVisible();
  await expect(page.getByRole('button',{name:'Reload Aether'})).toBeVisible();
  await expect(page.locator('details')).not.toHaveAttribute('open');
  await expect(page.locator('details p')).toContainText('Failed to fetch dynamically imported module');
  await page.unroute('**/features/fleet/graph/nodes.js');
  await page.getByRole('button',{name:'Reload Aether'}).click();
  await expect(page.getByRole('heading',{name:'Your agents. Within reach.'})).toBeVisible();
  await expect(page).toHaveURL(/#pair$/);
  await page.getByRole('button',{name:'Pair this device'}).click();
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:20000});
});

test('a typed code keeps the approval words up, from an address the app does not know',async({page})=>{
  await page.goto('/#code');
  await expect(page.getByRole('heading',{name:'Your agents. Within reach.'})).toBeVisible();
  await page.getByLabel('Pairing code').fill('aether-demo');
  await page.getByRole('button',{name:'Pair this device'}).click();
  await expect(page.getByRole('heading',{name:'Confirm on your host'})).toBeVisible();
  await expect(page.locator('.connection')).toHaveText('Not paired');
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:20000});
  await expect(page).toHaveURL(/#fleet$/);
  await expect(page.getByText('Device paired. You’re connected.')).toBeVisible();
  await page.goto('/#nowhere/at-all');
  await expect(page).toHaveURL(/#fleet$/);
  await expect(page.locator('.app-shell')).toHaveAttribute('data-screen','fleet');
  await expect(page.getByRole('heading',{name:'Page not found'})).toHaveCount(0);
});

test('Safari on an iPhone is told to install first and shown the code to type there',async({browser,baseURL})=>{
  const context=await browser.newContext({baseURL,userAgent:'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1'});
  try {
    const page=await context.newPage(); await page.goto('/#code=aether-demo');
    await expect(page.getByRole('heading',{name:'Add Aether to your Home Screen first'})).toBeVisible();
    await expect(page.locator('.pair-code')).toHaveText('aether-demo');
    await expect(page.getByRole('heading',{name:'Or pair in Safari'})).toBeVisible();
  } finally { await context.close(); }
});
