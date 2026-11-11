import test from 'node:test';
import assert from 'node:assert/strict';
import { chromium } from '@playwright/test';
import { createMockServer } from '../../dev/server.js';

async function mountHarness(page) {
  await page.evaluate(async () => {
    const { mountControls, mountSettings } = await import('/features/controls/index.js');
    const agent = { id: 'agt_test', name: 'Test worker', parent: null, role: 'worker', status: 'running', paused: false,
      model: 'codex/gpt-6-astra', effort: 'high', workspace: '/work', run: 'run_test', approval: 'ask',
      raw: { config: { instructions: 'Keep changes small', approval: { require_for: ['external'] } } } };
    const state = { outbox: [], connection: 'online', connectionDetail: '', role: 'operator', agents: [agent], lastSeq: 0,
      version: 'test', deviceName: 'Test phone', daemonId: 'daemon_test', error: null };
    const listeners = new Set();
    const sent = [];
    const store = { get: () => state, subscribe: fn => { listeners.add(fn); return () => listeners.delete(fn); },
      patch: patch => { Object.assign(state, patch); listeners.forEach(fn => fn()); }, refresh: async () => {} };
    const transport = { request: async command => {
      window.controlsTest.requests.push(command);
      const replies = { list_schedules: 'schedules', memories: 'memories', list_artifacts: 'artifacts', list_runs: 'runs',
        providers: 'providers', mcp_servers: 'mcp_servers', list_approval_rules: 'approval_rules', list_models: 'models', list_devices: 'devices',
        schedule_history: 'schedule_history', read_artifact: 'artifact_bytes', get_artifact: 'artifact', get_run_report: 'run_report',
        get_push_config: 'push_config', subscribe_push: 'push_config', unsubscribe_push: 'push_config' };
      if (command.cmd === 'subscribe_push') window.controlsTest.data.push_config.subscribed = true;
      if (command.cmd === 'unsubscribe_push') window.controlsTest.data.push_config.subscribed = false;
      const reply = replies[command.cmd] || 'ok';
      return { reply, data: window.controlsTest.data[reply] || [] };
    }, forget: async () => {}, connect: async () => {} };
    const context = { store, transport, command: async command => { sent.push(command); return { reply: 'ok' }; },
      confirm: async () => true, navigate: () => {}, toast: () => {}, preferences: { theme: 'system', motion: 'system' }, setPreference: () => {} };
    const root = document.createElement('main');
    document.body.replaceChildren(root);
    window.controlsTest = { state, sent, store, context, root, mountControls, mountSettings, listeners, data: {}, requests: [],
      cleanup: mountControls(root, context, 'agt_test') };
  });
}

async function verifyForms(page) {
  await page.getByLabel('Provider/model').fill('provider/family/model');
  await page.getByRole('button', { name: 'Change model', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.controlsTest.sent.at(-1)),
    { cmd: 'update_agent', id: 'agt_test', patch: { model: { provider: 'provider', model: 'family/model' } } });
  await page.getByRole('button', { name: 'Schedules', exact: true }).click();
  await page.getByLabel('Schedule name', { exact: true }).fill('Check CI');
  await page.getByLabel('Message for the agent').fill('Inspect new CI failures');
  await page.getByRole('button', { name: 'Create schedule', exact: true }).click();
  assert.equal(await page.evaluate(() => window.controlsTest.sent.at(-1).spec.target), 'agt_test');
  await page.getByRole('button', { name: 'Memories', exact: true }).click();
  await page.getByLabel('Memory', { exact: true }).fill('Cache inside the worktree');
  await page.getByRole('button', { name: 'Create memory', exact: true }).click();
  assert.equal(await page.evaluate(() => window.controlsTest.sent.at(-1).cmd), 'create_memory');
}

async function verifyObserver(page) {
  const before = await page.evaluate(() => window.controlsTest.sent.length);
  await page.evaluate(() => window.controlsTest.store.patch({ role: 'observer' }));
  assert.equal(await page.getByRole('button', { name: 'Create memory', exact: true }).isDisabled(), true);
  await page.evaluate(() => {
    const form = [...document.querySelectorAll('form')].find(node => node.textContent.includes('Create memory'));
    form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
  });
  await page.getByText('Observer devices are read only.', { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.controlsTest.sent.length), before);
  for (const label of ['Artifacts', 'Runs & reports', 'Diagnostics', 'Approval rules', 'Agent']) {
    await page.getByRole('button', { name: label, exact: true }).click();
    await page.locator('.controls-status').filter({ hasText: 'Working…' }).waitFor({ state: 'hidden' });
    assert.equal(await page.locator('.controls-status.controls-error').count(), 0, label);
  }
  assert.equal(await page.getByRole('button', { name: 'Interrupt', exact: true }).isDisabled(), true);
}

async function verifyResources(page) {
  await page.evaluate(() => {
    const artifact = { id: 'art_test', name: 'Report', content_type: 'text/plain', size_bytes: 14 };
    window.controlsTest.data = {
      schedules: [{ id: 'sch_test', name: 'Daily review', status: { state: 'active', next_due: '2026-10-13T09:00:00Z' } }],
      memories: [{ id: 'mem_test', kind: 'fact', text: 'Stored fact', pinned: false }],
      artifacts: [artifact], artifact, artifact_bytes: { artifact, base64: btoa('<b>literal</b>'), truncated: true },
      runs: [{ run: 'run_test', objective: 'Test run' }], run_report: { run: 'run_test', body: { summary: 'No blockers' } },
      approval_rules: [{ id: 'rule_test', decision: 'denied', scope: { scope: 'agent', id: 'agt_test' } }],
    };
  });
  await page.getByRole('button', { name: 'Schedules', exact: true }).click();
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.controlsTest.sent.at(-1)), { cmd: 'update_schedule', id: 'sch_test', action: 'pause' });
  await page.getByRole('button', { name: 'History', exact: true }).click();
  await page.getByText('No occurrences recorded.').waitFor();
  await page.getByRole('button', { name: 'Artifacts', exact: true }).click();
  await page.getByRole('button', { name: 'Preview file', exact: true }).click();
  await page.getByText('<b>literal</b>', { exact: true }).waitFor();
  assert.equal(await page.locator('.controls-preview b').count(), 0);
  assert.match(await page.getByRole('link', { name: 'Download partial bytes' }).getAttribute('download'), /\.partial$/);
  await page.getByRole('button', { name: 'Runs & reports', exact: true }).click();
  await page.getByRole('button', { name: 'Read run report', exact: true }).click();
  await page.getByText('No blockers', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Approval rules', exact: true }).click();
  await page.getByRole('button', { name: 'Revoke rule', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.controlsTest.sent.at(-1)), { cmd: 'revoke_approval_rule', id: 'rule_test' });
  await page.getByRole('button', { name: 'Memories', exact: true }).click();
  await page.getByRole('button', { name: 'Pin', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.controlsTest.sent.at(-1)), { cmd: 'pin_memory', id: 'mem_test', pinned: true });
  await page.getByRole('button', { name: 'Forget', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.controlsTest.sent.at(-1)), { cmd: 'forget_memory', id: 'mem_test' });
}

async function verifySettings(page) {
  await page.evaluate(() => { const t = window.controlsTest; t.cleanup(); t.cleanup = t.mountSettings(t.root, t.context); });
  assert.equal(await page.getByRole('button', { name: 'Enable notifications', exact: true }).isDisabled(), false);
  assert.equal(await page.getByRole('button', { name: 'Save appearance', exact: true }).isDisabled(), false);
  assert.equal(await page.getByRole('button', { name: 'Load paired devices', exact: true }).isDisabled(), true);
  await page.getByText('Operators can view paired devices. Changing or revoking access requires the Aether host.').waitFor();
  await verifyDevices(page);
  await verifyNotifications(page);
  await page.evaluate(() => window.controlsTest.store.patch({ connection: 'offline' }));
  assert.equal(await page.getByRole('button', { name: 'Enable notifications', exact: true }).isDisabled(), true);
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
  await page.screenshot({ path: 'node_modules/controls-settings-mobile.png', fullPage: true });
  await page.evaluate(() => window.controlsTest.cleanup());
  assert.equal(await page.evaluate(() => window.controlsTest.listeners.size), 0);
}

async function verifyDevices(page) {
  await page.evaluate(() => {
    window.controlsTest.data.devices = [{ id: 'dev_other', name: 'Paired laptop', role: 'operator', current: false,
      created_at: '2026-10-12T08:00:00Z', last_seen_at: null, fingerprint: 'must-not-display' }];
    window.controlsTest.store.patch({ role: 'operator' });
  });
  await page.getByRole('button', { name: 'Load paired devices', exact: true }).click();
  await page.getByRole('heading', { name: 'Paired laptop', exact: true }).waitFor();
  assert.equal(await page.getByText('must-not-display', { exact: true }).count(), 0);
  assert.equal(await page.getByRole('button', { name: 'Revoke device', exact: true }).count(), 0);
  await page.evaluate(() => window.controlsTest.store.patch({ role: 'observer' }));
  assert.equal(await page.getByRole('heading', { name: 'Paired laptop', exact: true }).count(), 0);
}

async function verifyNotifications(page) {
  await page.evaluate(() => {
    localStorage.setItem('aether.push.disabled', 'true');
    Object.defineProperty(globalThis, 'Notification', { configurable: true, value: {
      permission: 'granted', requestPermission: async () => 'granted',
    } });
    window.controlsTest.data.push_config = { enabled: true, vapid_public_key: btoa('\x04' + '\x01'.repeat(64)), subscribed: false, previews: false, kinds: [] };
    const subscription = { toJSON: () => ({ endpoint: 'https://push.example/test', keys: { p256dh: 'public-key', auth: 'auth-key' } }), unsubscribe: async () => true };
    Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: {
      getRegistration: async () => ({ active: true, pushManager: { getSubscription: async () => subscription } }),
    } });
  });
  await page.getByRole('button', { name: 'Enable notifications', exact: true }).click();
  await page.getByText('The daemon accepted this browser’s notification subscription.').waitFor();
  assert.equal(await page.evaluate(() => localStorage.getItem('aether.push.disabled')), null);
  const subscription = await page.evaluate(() => window.controlsTest.requests.find(command => command.cmd === 'subscribe_push'));
  assert.deepEqual(subscription.subscription, { endpoint: 'https://push.example/test', p256dh: 'public-key', auth: 'auth-key' });
  await page.getByRole('button', { name: 'Send test notification', exact: true }).click();
  assert.equal(await page.evaluate(() => window.controlsTest.sent.at(-1).cmd), 'test_push');
  await verifyFailedPush(page);
  await verifyFailedNotificationDisable(page);
  await page.getByRole('button', { name: 'Disable notifications', exact: true }).click();
  await page.getByText('Notification subscription removed.', { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => localStorage.getItem('aether.push.disabled')), 'true');
  assert.equal(await page.evaluate(() => window.controlsTest.requests.at(-2).cmd), 'unsubscribe_push');
  await page.evaluate(() => { navigator.serviceWorker.getRegistration = async () => undefined; });
  await page.getByRole('button', { name: 'Disable notifications', exact: true }).click();
  assert.equal(await page.locator('.controls-error').count(), 0);
}

async function verifyFailedPush(page) {
  await page.evaluate(() => {
    const context=window.controlsTest.context;const original=context.command;
    window.controlsTest.restorePush=()=>{context.command=original;};
    context.command=async command=>{
      if(command.cmd!=='test_push')return original(command);
      window.controlsTest.data.push_config.subscribed=false;
      throw Object.assign(new Error('Push service returned 410; subscription removed'),{code:'unavailable'});
    };
  });
  await page.getByRole('button',{name:'Send test notification',exact:true}).click();
  await page.getByText(/Test notification was not delivered: Push service returned 410; subscription removed/).waitFor();
  assert.equal(await page.getByText('not subscribed',{exact:true}).isVisible(),true);
  await page.evaluate(()=>window.controlsTest.restorePush());
  await verifyRefusedPushEndpoint(page);
}

async function verifyRefusedPushEndpoint(page) {
  await page.evaluate(()=>{
    const context=window.controlsTest.context;const original=context.transport.request;
    window.controlsTest.restorePush=()=>{context.transport.request=original;};
    context.transport.request=async command=>{
      if(command.cmd==='subscribe_push')throw Object.assign(new Error('Push service host is not allowed'),{code:'invalid_argument'});
      return original(command);
    };
  });
  await page.getByRole('button',{name:'Enable notifications',exact:true}).click();
  await page.getByText(/daemon registration failed: Push service host is not allowed/).waitFor();
  await page.evaluate(()=>window.controlsTest.restorePush());
}

async function verifyFailedNotificationDisable(page) {
  await page.evaluate(() => {
    const context = window.controlsTest.context;
    const request = context.transport.request;
    window.controlsTest.restoreRequest = () => { context.transport.request = request; };
    context.transport.request = command => {
      if (command.cmd === 'unsubscribe_push') throw new Error('Server refused removal');
      return request(command);
    };
  });
  await page.getByRole('button', { name: 'Disable notifications', exact: true }).click();
  await page.getByText('Server refused removal', { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => localStorage.getItem('aether.push.disabled')), null);
  await page.evaluate(() => window.controlsTest.restoreRequest());
}

test('controls mobile forms, observer guards, resource tabs and settings cleanup', {
  skip: process.env.RUN_CONTROLS_BROWSER !== '1', timeout: 90000,
}, async () => {
  const server = createMockServer();
  await new Promise(resolve => server.http.listen(0, '127.0.0.1', resolve));
  const address = server.http.address();
  assert.ok(address && typeof address === 'object');
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    page.setDefaultTimeout(10000);
    await page.route('**/app.js', route => route.fulfill({ contentType: 'text/javascript', body: '' }));
    await page.goto(`http://127.0.0.1:${address.port}/`);
    await mountHarness(page);
    await verifyForms(page);
    await verifyResources(page);
    await verifySummaries(page);
    await verifyObserver(page);
    await verifySettings(page);
  } finally { await browser?.close(); await server.close(); }
});

async function verifySummaries(page) {
  await page.getByRole('button', { name: 'Agent', exact: true }).click();
  await page.getByText('Inspect original record', { exact: true }).click();
  await page.evaluate(() => window.controlsTest.store.patch({ lastSeq: 1 }));
  assert.equal(await page.locator('.controls-record').evaluate(node => node.open), true);
  const usage = { input_tokens: 12345, output_tokens: 0, cost_usd: 7.289999999999999, cost_complete: false };
  await page.evaluate(usage => {
    window.controlsTest.data.runs = [{ run: 'run_test', agents: 7, agents_live: 2, started_at: '2026-10-12T09:00:00Z', usage }];
    window.controlsTest.data.providers = [{ name: 'Codex', configured: false, enabled: false, health: { running: 0 } }];
    window.controlsTest.data.mcp_servers = [{ name: 'Local tools', state: 'failed', error: 'Connection refused', tools: [] }];
  }, usage);
  await page.getByRole('button', { name: 'Runs & reports', exact: true }).click();
  await page.getByRole('heading', { name: 'Test worker', exact: true }).waitFor();
  assert.equal(await page.getByText('$7.29', { exact: true }).isVisible(), true);
  assert.equal(await page.getByText('12,345', { exact: true }).isVisible(), true);
  await page.getByText('Partial coverage · total cost may be higher. Cost source unknown.', { exact: true }).waitFor();
  const raw = page.locator('.controls-record').filter({ has: page.getByText('Inspect original record', { exact: true }) });
  assert.equal(await raw.evaluate(node => node.open), false);
  await raw.locator('summary').focus();
  await page.keyboard.press('Enter');
  assert.deepEqual(JSON.parse(await raw.locator('pre').innerText()).usage, usage);
  await page.getByText('More usage details', { exact: true }).click();
  assert.match(await page.locator('.controls-usage').innerText(), /Reasoning tokens\s+Not reported/);
  await page.evaluate(() => { window.controlsTest.data.runs = [{ run: 'missing_run' }]; });
  await page.getByRole('button', { name: 'Refresh runs', exact: true }).click();
  await page.getByText('Usage not reported. Cost and token counts are unknown.', { exact: true }).waitFor();
  assert.equal(await page.getByRole('heading', { name: 'Stored run', exact: true }).isVisible(), true);
  await page.getByRole('button', { name: 'Diagnostics', exact: true }).click();
  await page.getByText('Not configured', { exact: true }).waitFor();
  assert.equal(await page.getByText('Disabled', { exact: true }).isVisible(), true);
  assert.equal(await page.getByText('Connection refused', { exact: true }).isVisible(), true);
  await page.getByRole('button', { name: 'Memories', exact: true }).click();
}
