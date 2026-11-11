import {test,expect} from '@playwright/test';
import {createMockServer} from '../dev/server.js';

for (const viewport of [{width:390,height:844,scale:100},{width:360,height:640,scale:100},{width:320,height:844,scale:200}]) {
  test(`primary actions stay reachable at ${viewport.width}×${viewport.height}, ${viewport.scale}% text`,async({page})=>{
    test.setTimeout(90000);
    await page.setViewportSize(viewport);
    const server=createMockServer();
    await new Promise(resolve=>server.http.listen(0,'127.0.0.1',()=>resolve(undefined)));
    const address=server.http.address();
    if (!address || typeof address==='string') throw new Error('Expected an isolated mock listener');
    const origin=`http://127.0.0.1:${address.port}`;
    try {
      await page.goto(`${origin}/#pair=aether-demo`);
      await checkActions(page);
      const deviceName=page.getByRole('textbox',{name:'Device name'});
      await deviceName.fill('Phone\u200b');
      await page.getByRole('button',{name:'Pair this device'}).click();
      expect(await deviceName.evaluate(input=>/** @type {HTMLInputElement} */ (input).validationMessage)).toMatch(/invisible/);
      await deviceName.fill('Phone 📱');
      await page.getByRole('button',{name:'Pair this device'}).click();
      await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:20000});
      await page.evaluate(scale=>{document.documentElement.style.fontSize=`${scale}%`;},viewport.scale);
      const id='agt_00000000000000000000000001';
      const routes=['fleet','attention','activity','new','settings',`agent/${id}`,`controls/${id}`];
      for (const panel of ['schedules','memories','artifacts','runs','diagnostics','rules']) routes.push(`controls/${id}?panel=${panel}`);
      for (const route of routes) {
        await page.goto(`${origin}/#${route}`);
        await expect(page.locator('main h1')).toBeVisible();
        await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible();
        await checkActions(page);
        const nav=page.getByRole('navigation',{name:'Main navigation'});
        if (await nav.isVisible()) {
          const mainBox=await page.locator('main').boundingBox();const navBox=await nav.boundingBox();
          expect(mainBox && navBox).toBeTruthy();
          expect((mainBox?.y ?? 0)+(mainBox?.height ?? 0)).toBeLessThan(navBox?.y ?? 0);
        }
        if (route==='attention') {
          await expect(page.locator('.fleet-attention-card[data-kind="approval"] h2')).toHaveText('Push a branch');
          await expect(page.locator('.attention-exact')).not.toHaveAttribute('open');
        }
      }
    } finally {await server.close();}
  });
}

/** @param {import('@playwright/test').Page} page */
async function checkActions(page) {
  const actions=page.locator('main .button.primary:visible');
  for (let index=0;index<await actions.count();index++) {
    const action=actions.nth(index);
    await action.evaluate(node=>node.scrollIntoView({block:'center',behavior:'instant'}));
    await expect(action).toBeInViewport({ratio:1});
    const obstruction=await action.evaluate(node=>{
      const area=node.getBoundingClientRect();
      const points=[[area.left+8,area.top+8],[area.right-8,area.top+8],[area.left+8,area.bottom-8],[area.right-8,area.bottom-8]];
      return points.map(([x,y])=>{
        const hit=document.elementFromPoint(x,y);
        return hit && (hit===node || node.contains(hit)) ? '' : hit?.className || 'outside the viewport';
      }).find(Boolean) || '';
    });
    expect(obstruction,`Primary action ${await action.textContent()} is covered`).toBe('');
  }
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
}
