import {test,expect} from '@playwright/test';
import {createMockServer} from '../dev/server.js';

const FAILED=['agt_00000000000000000000000006','agt_00000000000000000000000008','agt_00000000000000000000000009'];
const BLOCKED='agt_00000000000000000000000004';

/** Two extra failed agents beside the fixture's failed, blocked, running and completed ones. */
function serverWithFailures() {
  const server=createMockServer();
  const failed=server.state.agents.find(agent=>agent.id===FAILED[0]);
  if (!failed) throw new Error('The fixtures must include a failed agent');
  for (const [id,name] of [[FAILED[1],'Nightly build triage'],[FAILED[2],'Flaky test bisect']]) {
    server.state.agents.push({...structuredClone(failed),id,name});
  }
  return server;
}

/** @param {import('@playwright/test').Page} page @param {ReturnType<typeof createMockServer>} server @param {string} [secret] */
async function openAttention(page,server,secret='aether-demo') {
  await new Promise(resolve=>server.http.listen(0,'127.0.0.1',()=>resolve(undefined)));
  const address=server.http.address();
  if (!address || typeof address==='string') throw new Error('Expected an isolated mock listener');
  const origin=`http://127.0.0.1:${address.port}`;
  await page.goto(`${origin}/#pair=${secret}`);
  await page.getByRole('button',{name:'Pair this device'}).click();
  await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:20000});
  await page.goto(`${origin}/#attention`);
  await expect(page.locator('.fleet-attention-card').first()).toBeVisible();
}

/** @param {ReturnType<typeof createMockServer>} server */
function archivedIds(server) {
  return server.state.agents.filter(agent=>/** @type {{archived?:boolean}} */ (agent).archived===true).map(agent=>agent.id).sort();
}

test('an operator sees dismiss only on failed and blocked cards; an observer sees none',async({page})=>{
  const server=serverWithFailures();
  try {
    await openAttention(page,server);
    for (const kind of ['agent_failed','agent_blocked']) {
      for (const card of await page.locator(`.fleet-attention-card[data-kind="${kind}"]`).all()) await expect(card.getByRole('button',{name:'Dismiss…'})).toBeVisible();
    }
    for (const kind of ['approval','question']) await expect(page.locator(`.fleet-attention-card[data-kind="${kind}"]`).getByRole('button',{name:'Dismiss…'})).toHaveCount(0);
    await expect(page.getByRole('button',{name:'Clear all failed (3)'})).toBeVisible();
  } finally {await server.close();}
  const observed=serverWithFailures();
  try {
    await page.context().clearCookies(); await page.evaluate(()=>{localStorage.clear();});
    await openAttention(page,observed,'aether-observer');
    await expect(page.locator('.fleet-attention-card[data-kind="agent_failed"]').first()).toBeVisible();
    await expect(page.getByRole('button',{name:/Dismiss|Clear all failed/})).toHaveCount(0);
  } finally {await observed.close();}
});

test('clear all failed archives exactly the failed agents shown and leaves blocked and running ones',async({page})=>{
  const server=serverWithFailures();
  try {
    await openAttention(page,server);
    await page.getByRole('button',{name:'Clear all failed (3)'}).click();
    await expect(page.getByRole('heading',{name:'Clear 3 failed agents?'})).toBeVisible();
    await page.getByRole('button',{name:'Confirm'}).click();
    await expect(page.getByText('Cleared 3 failed agents')).toBeVisible();
    await expect(page.locator('.fleet-attention-card[data-kind="agent_failed"]')).toHaveCount(0);
    await expect(page.locator('.fleet-attention-card[data-kind="agent_blocked"]')).toHaveCount(1);
    await expect(page.getByRole('button',{name:/Clear all failed/})).toHaveCount(0);
    expect(archivedIds(server)).toEqual([...FAILED].sort());
    expect(server.state.agents.find(agent=>agent.id===BLOCKED)?.status.state).toBe('blocked');
    await page.locator('.fleet-attention-card[data-kind="agent_blocked"]').getByRole('button',{name:'Dismiss…'}).click();
    await expect(page.getByRole('heading',{name:'Stop and dismiss Browser checks?'})).toBeVisible();
    await page.getByRole('button',{name:'Confirm'}).click();
    await expect(page.getByText('Dismissed Browser checks')).toBeVisible();
    await expect(page.locator('.fleet-attention-card[data-kind="agent_blocked"]')).toHaveCount(0);
    expect(server.state.agents.find(agent=>agent.id===BLOCKED)?.status.state).toBe('cancelled');
    expect(archivedIds(server)).toEqual([...FAILED,BLOCKED].sort());
  } finally {await server.close();}
});

test('a failed archive shows the error and keeps the card until the daemon confirms',async({page})=>{
  const server=serverWithFailures();
  const execute=server.execute.bind(server);
  server.execute=(command,session,req)=>{
    if (command.cmd==='archive') throw new Error('store is read only');
    return execute(command,session,req);
  };
  try {
    await openAttention(page,server);
    const card=page.locator(`.fleet-attention-card[data-attention="attention-agent_failed-${FAILED[0]}"]`);
    await card.getByRole('button',{name:'Dismiss…'}).click();
    await page.getByRole('button',{name:'Confirm'}).click();
    await expect(page.getByText(/Could not dismiss CI investigation: .*store is read only/)).toBeVisible();
    await expect(card.getByRole('alert')).toContainText('store is read only');
    await expect(card.getByRole('button',{name:'Dismiss…'})).toBeEnabled();
    expect(archivedIds(server)).toEqual([]);
    server.execute=execute;
    await card.getByRole('button',{name:'Dismiss…'}).click();
    await page.getByRole('button',{name:'Confirm'}).click();
    await expect(card).toHaveCount(0);
    expect(archivedIds(server)).toEqual([FAILED[0]]);
  } finally {await server.close();}
});
