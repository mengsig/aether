import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium, expect } from '@playwright/test';
import { createMockServer } from '../../dev/server.js';

async function captureViews(page, baseURL) {
  const destination = 'node_modules/controls-polish';
  await mkdir(destination, { recursive: true });
  await page.goto(`${baseURL}/#pair=aether-demo`);
  await page.getByRole('button', { name: 'Pair this device', exact: true }).click();
  await page.locator('.connection[data-state="online"]').waitFor();
  await page.locator('.toast').waitFor({ state: 'hidden' });
  for (const theme of ['light', 'dark']) {
    await page.evaluate(theme => {
      localStorage.setItem('aether.preferences', JSON.stringify({ theme, motion: 'reduced' }));
    }, theme);
    await page.reload();
    await page.locator('.connection[data-state="online"]').waitFor();
    for (const panel of ['agent', 'schedules', 'memories', 'artifacts', 'runs', 'diagnostics', 'rules', 'settings']) {
      const route = panel === 'settings' ? 'settings' : `controls/agt_00000000000000000000000001?panel=${panel}`;
      await page.goto(`${baseURL}/#${route}`);
      await page.getByRole('heading', { name: panel === 'settings' ? 'Settings' : 'Agent controls', exact: true }).waitFor();
      if (panel !== 'settings') await page.locator(`[data-panel="${panel}"][aria-pressed="true"]`).waitFor();
      await page.locator('.controls-status').filter({ hasText: 'Working…' }).waitFor({ state: 'hidden' });
      assert.equal(await page.locator('.controls-error').count(), 0, `${panel} errors`);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `${panel} overflow`);
      await page.screenshot({ path: `${destination}/${panel}-${theme}.png`, fullPage: true, animations: 'disabled' });
    }
  }
  await page.goto(`${baseURL}/#controls/agt_00000000000000000000000001?panel=runs`);
  await page.getByRole('button', { name: 'Read run report', exact: true }).click();
  await page.getByRole('heading', { name: 'Remote dashboard release', exact: true }).waitFor();
  assert.equal(await page.locator('.controls-record[open]').count(), 0);
  await page.evaluate(() => { window.scrollTo(0, 0); });
  await page.screenshot({ path: `${destination}/run-report-dark.png`, fullPage: true, animations: 'disabled' });
  await page.evaluate(() => { document.documentElement.style.fontSize = '200%'; });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await expect.poll(() => page.locator('.controls-details').first().evaluate(node => getComputedStyle(node).gridTemplateColumns.split(' ').length)).toBe(1);
  await page.screenshot({ path: `${destination}/runs-large-text.png`, fullPage: true, animations: 'disabled' });
}

test('controls phone views render in both themes with exact records collapsed', {
  skip: process.env.RUN_CONTROLS_BROWSER !== '1', timeout: 90000,
}, async () => {
  const server = createMockServer();
  await new Promise(resolve => server.http.listen(0, '127.0.0.1', resolve));
  const address = server.http.address();
  assert.ok(address && typeof address === 'object');
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage({ viewport: { width: 390, height: 844 }, reducedMotion: 'reduce' });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await captureViews(page, `http://127.0.0.1:${address.port}`);
    assert.deepEqual(errors, []);
  } finally { await browser?.close(); await server.close(); }
});
