import {expect} from '@playwright/test';

/** @param {import('@playwright/test').Page} page @param {import('./browser.js').Fixture} fixture */
export async function checkPushBoundary(page,fixture) {
  await page.goto(`${fixture.url}/#settings`);
  await expect(page.getByText('Live operator phone',{exact:true}).first()).toBeVisible();
  const result=await page.evaluate(async()=>{
    const modulePath='/core/index.js';
    const {createTransport}=/** @type {typeof import('../../core/index.js')} */ (await import(modulePath));
    const transport=createTransport();
    /** @param {import('../../types.js').Command} command */
    const refusal=async command=>{
      try {await transport.request(command);return {code:'accepted',message:''};}
      catch (error) {
        if (!(error instanceof Error) || !('code' in error)) throw error;
        return {code:String(error.code),message:error.message};
      }
    };
    try {
      await transport.connect();
      const config=await transport.request({cmd:'get_push_config'});
      const test=await refusal({cmd:'test_push'});
      const key=await crypto.subtle.generateKey({name:'ECDH',namedCurve:'P-256'},true,['deriveBits']);
      /** @param {Uint8Array} bytes */
      const encode=bytes=>btoa(String.fromCharCode(...bytes)).replaceAll('+','-').replaceAll('/','_').replaceAll('=','');
      const publicKey=new Uint8Array(await crypto.subtle.exportKey('raw',key.publicKey));
      const local=await refusal({cmd:'subscribe_push',subscription:{endpoint:'https://127.0.0.1:443/fake-push',p256dh:encode(publicKey),auth:encode(crypto.getRandomValues(new Uint8Array(16)))},kinds:[]});
      return {config,test,local};
    } finally {transport.disconnect();}
  });
  expect(result.config.reply).toBe('push_config');
  expect(result.test.code).toBe('unavailable');
  expect(result.local.code).not.toBe('accepted');
  console.log(`Live push: disabled delivery verified; local subscription refused (${result.local.code}: ${result.local.message}). No external push service contacted.`);
}
