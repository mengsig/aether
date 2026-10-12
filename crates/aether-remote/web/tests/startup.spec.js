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
