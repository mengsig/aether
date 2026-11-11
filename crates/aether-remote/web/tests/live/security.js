import {expect} from '@playwright/test';
import {createECDH} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';
import {until} from './host.js';

/** @typedef {import('./browser.js').Fixture} Fixture */
/** @typedef {{id:string,code:string}} Pairing */
/** @typedef {{status:number,retryAfter:string|null,body:any}} Claim */

/** @param {import('@playwright/test').Browser} browser @param {Fixture} fixture */
export async function checkSecurity(browser,fixture) {
  expect(new URL(fixture.url).hostname).toBe('127.0.0.1');
  expect(fixture.socket).toMatch(/^\/var\/tmp\/aw-[^/]+\/daemon\.sock$/);
  await checkClaims(fixture);
  const loopback = await checkBrowserOrigin(browser,fixture,fixture.url);
  const localhost = new URL(fixture.url); localhost.hostname='localhost';
  const named = await checkBrowserOrigin(browser,fixture,localhost.origin);
  expect(named).toBe(loopback);
  console.log(`Live security: ${browser.browserType().name()} authenticated at both loopback origins; CSP: ${loopback}`);
}

/** @param {Fixture} fixture @returns {Promise<Pairing>} */
async function startPairing(fixture) {
  const reply = await fixture.request({cmd:'start_pairing',role:'observer'});
  expect(reply.reply).toBe('pairing');
  expect(reply.data.state).toBe('open');
  expect(reply.data.id).toEqual(expect.any(String));
  expect(reply.data.code).toMatch(/^[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$/);
  return reply.data;
}

/** @param {Fixture} fixture @param {string} publicKey @param {string} code @param {string} name @returns {Promise<Claim>} */
async function claim(fixture,publicKey,code,name) {
  expect(publicKey).toMatch(/^[A-Za-z0-9_-]{87}$/);
  expect(new URL(fixture.url).hostname).toBe('127.0.0.1');
  const response = await fetch(`${fixture.url}/api/pair/claim`,{
    method:'POST',headers:{'Content-Type':'application/json',Origin:fixture.url},
    body:JSON.stringify({code,public_key:publicKey,device_name:name}),signal:AbortSignal.timeout(10000),
  });
  const body = await response.json();
  expect(body).toBeTruthy();
  return {status:response.status,retryAfter:response.headers.get('retry-after'),body};
}

/** @param {Fixture} fixture */
async function checkClaims(fixture) {
  const key = createECDH('prime256v1'); key.generateKeys();
  const publicKey = key.getPublicKey().toString('base64url');
  const pairing = await startPairing(fixture);
  try {
    await checkNameRefusals(fixture,publicKey,pairing);
    await checkGuessLimit(fixture,publicKey,pairing);
  } finally {
    const cancelled = await fixture.request({cmd:'cancel_pairing',id:pairing.id});
    expect(cancelled.data.state).toBe('cancelled');
  }
  for (const [input,normalized] of [
    ['  Live\u00a0\u3000thin\u2009\u2028\u2029 phone  ','Live thin phone'],
    ['📱'.repeat(64),'📱'.repeat(64)],
  ]) {
    const opened = await startPairing(fixture);
    try {
      const accepted = await claim(fixture,publicKey,opened.code,input);
      expect(accepted.status).toBe(200); expect(accepted.body.pairing).toBe(opened.id);
      const state = await fixture.request({cmd:'get_pairing',id:opened.id});
      expect(state.data.state).toBe('claimed'); expect(state.data.candidate.name).toBe(normalized);
    } finally {await fixture.request({cmd:'cancel_pairing',id:opened.id});}
  }
  console.log('Live security: Unicode name normalization/refusals and bounded wrong-code rate limit passed.');
}

/** @param {Fixture} fixture @param {string} publicKey @param {Pairing} pairing */
async function checkNameRefusals(fixture,publicKey,pairing) {
  const invisible = ['\t','\n','\u0085','\u200b','\u200c','\u200d','\u00ad','\ufeff',
    '\u202a','\u202e','\u2066','\u200e','\ufe0f','\u{e0100}','\u115f','\u3164',
    '\u2800','\u{e0061}','\ue000','\u{f0000}','\ufdd0','\u{10ffff}'];
  const names = ['', ' \u00a0 ', 'x'.repeat(65), '📱'.repeat(65), ...invisible.map(value=>`Phone${value}name`)];
  for (const name of names) {
    const refused = await claim(fixture,publicKey,pairing.code,name);
    expect(refused.status,`name ${JSON.stringify(name)}`).toBe(400);
    expect(refused.body.error.code).toBe('bad_request');
  }
  const state = await fixture.request({cmd:'get_pairing',id:pairing.id});
  expect(state.data.state).toBe('open');
  expect(state.data.candidate).toBeUndefined();
}

/** @param {Fixture} fixture @param {string} publicKey @param {Pairing} pairing */
async function checkGuessLimit(fixture,publicKey,pairing) {
  const wrong = pairing.code === '0000-0000' ? '1111-1111' : '0000-0000';
  const malformed = await claim(fixture,publicKey,'not-a-code','Security phone');
  expect(malformed.status).toBe(403); expect(malformed.body.error.code).toBe('invalid_code');
  const first = await claim(fixture,publicKey,wrong,'Security phone');
  expect(first.status).toBe(403); expect(first.body.error.code).toBe('invalid_code');
  const burst = await Promise.all(Array.from({length:16},()=>claim(fixture,publicKey,wrong,'Security phone')));
  expect(burst.some(reply=>reply.status===429)).toBe(true);
  for (const reply of burst) {
    expect([403,429]).toContain(reply.status);
    expect(reply.body.error.code).toBe(reply.status===429?'rate_limited':'invalid_code');
    if (reply.status===429) expect(reply.retryAfter).toBe('1');
  }
  const state = await fixture.request({cmd:'get_pairing',id:pairing.id});
  expect(state.data.state).toBe('open'); expect(state.data.candidate).toBeUndefined();
  await delay(1100);
  const valid = await claim(fixture,publicKey,pairing.code,'After wrong guesses');
  expect(valid.status).toBe(200); expect(valid.body.pairing).toBe(pairing.id);
}

/** @param {import('@playwright/test').Browser} browser @param {Fixture} fixture @param {string} origin */
async function checkBrowserOrigin(browser,fixture,origin) {
  const context = await browser.newContext({serviceWorkers:'block'});
  /** @type {Pairing|undefined} */ let pairing;
  let deviceId;
  try {
    pairing = await startPairing(fixture);
    const pairingId = pairing.id;
    const page = await context.newPage();
    /** @type {string[]} */ const errors=[];
    /** @type {any[]} */ const welcomes=[];
    page.on('pageerror',error=>errors.push(error.message));
    page.on('console',message=>{
      if (/content.security.policy|\bcsp\b|connect-src|violat.*(?:directive|policy)/i.test(message.text())) errors.push(message.text());
    });
    page.on('websocket',socket=>socket.on('framereceived',frame=>{
      const value = JSON.parse(String(frame.payload));
      if (value.result?.Ok?.reply==='welcome') welcomes.push(value.result.Ok.data);
    }));
    const response = await page.goto(`${origin}/#pair=${pairing.code}`);
    expect(response?.status()).toBe(200);
    const csp = response?.headers()['content-security-policy'];
    expect(csp).toEqual(expect.any(String));
    expect(csp?.split(';').map(value=>value.trim()).filter(value=>value.startsWith('connect-src'))).toEqual(["connect-src 'self'"]);
    await page.getByRole('textbox',{name:'Device name'}).fill(`Security ${new URL(origin).hostname}`);
    await page.getByRole('button',{name:'Pair this device'}).click();
    const claimed = await until(()=>fixture.request({cmd:'get_pairing',id:pairingId}),reply=>reply.data.state==='claimed','security browser claim');
    await expect(page.getByText(claimed.data.candidate.phrase,{exact:true})).toBeVisible();
    const enrolled = await fixture.request({cmd:'confirm_pairing',id:pairing.id,accept:true});
    deviceId = enrolled.data.device.id;
    expect(enrolled.data.state).toBe('enrolled'); expect(deviceId).toEqual(expect.any(String));
    await expect(page.locator('.connection[data-state="online"]:visible')).toBeVisible({timeout:20000});
    expect(welcomes.length).toBeGreaterThan(0);
    expect(welcomes[0].device.id).toBe(deviceId); expect(welcomes[0].protocol_version).toBe(18);
    await checkRename(fixture,deviceId);
    expect(errors,`${origin} CSP/browser errors`).toEqual([]);
    console.log(`Live security: ${origin} protocol-18 welcome; CSP ${csp}`);
    return csp;
  } finally {
    try {
      if (deviceId) {
        const revoked = await fixture.request({cmd:'revoke_device',id:deviceId,reason:'Security acceptance complete'});
        expect(revoked.data.revoked_at).toBeTruthy();
      } else if (pairing) {await fixture.request({cmd:'cancel_pairing',id:pairing.id});}
    } finally {await context.close();}
  }
}

/** @param {Fixture} fixture @param {string} id */
async function checkRename(fixture,id) {
  expect(id).toMatch(/^dev_[0-9A-Z]{26}$/);
  const renamed = await fixture.request({cmd:'update_device',id,name:'  Security\u00a0\u3000phone 📱  '});
  expect(renamed.data.name).toBe('Security phone 📱');
  for (const name of ['\tPhone','Phone\u202e','Phone\u200d','Phone\ufe0f','Phone\ue000','📱'.repeat(65)]) {
    await expect(fixture.request({cmd:'update_device',id,name})).rejects.toThrow(/invalid_argument/);
  }
  const devices = await fixture.request({cmd:'list_devices'});
  const retained = devices.data.find((/** @type {{id:string}} */ device)=>device.id===id);
  expect(retained).toBeTruthy(); expect(retained.name).toBe('Security phone 📱');
}
