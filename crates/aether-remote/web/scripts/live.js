import path from 'node:path';
import {mkdir} from 'node:fs/promises';
import {liveFixture} from '../tests/live/fixture.js';

const web=path.resolve(import.meta.dirname,'..');
const binary=process.argv[2] || process.env.AETHER_BINARY;
if (!binary) throw new Error('Usage: npm run test:live -- /absolute/path/to/aether');
process.env.TMPDIR=path.join(web,'node_modules','.live-tmp');
process.env.PLAYWRIGHT_BROWSERS_PATH=path.join(web,'node_modules','.browsers');
await mkdir(process.env.TMPDIR,{recursive:true});
process.chdir(web);
const {chromium}=await import('@playwright/test');
const {pairBrowser,startAgent,captureScreens,seedCaptureAttention,captureFleetSize}=await import('../tests/live/browser.js');
const {seedChat,checkChat}=await import('../tests/live/chat.js');
const {checkObserver,checkRecovery}=await import('../tests/live/recovery.js');
const {checkResources}=await import('../tests/live/resources.js');
const {checkDecisions}=await import('../tests/live/decisions.js');
const {checkPushBoundary}=await import('../tests/live/push.js');
const {checkSecurity}=await import('../tests/live/security.js');
const {checkLostAcknowledgement}=await import('../tests/live/submissions.js');
const fixture=await liveFixture(path.resolve(binary));
try {
  const browser=await chromium.launch();
  try {
    const context=await browser.newContext({viewport:{width:390,height:844},deviceScaleFactor:1});
    const page=await context.newPage();
    /** @type {string[]} */ const errors=[];
    page.on('pageerror',error=>errors.push(error.message));
    console.log('Live: pairing and root creation');
    await pairBrowser(page,fixture,'operator');await startAgent(page,fixture);
    await captureFleetSize(page,fixture,1);
    const agent=await seedChat(fixture);
    console.log('Live: streaming, completion and delegation');
    await checkChat(page,fixture,agent);
    console.log('Live: controls, schedules, memories, artifacts and reports');
    const resourceAgent=await checkResources(page,fixture,agent);
    console.log('Live: exact approvals and conditional questions');
    await checkDecisions(page,fixture);
    console.log('Live: observer permissions and revocation');
    await checkObserver(browser,fixture);
    await checkPushBoundary(page,fixture);
    console.log('Live: restart, drafts and offline recovery');
    await checkRecovery(page,fixture,agent);
    console.log('Live: lost acknowledgement recovery');
    await checkLostAcknowledgement(page,fixture,agent);
    console.log('Live: connection security and both loopback origins');
    await checkSecurity(browser,fixture);
    console.log('Live: all screens in light and dark');
    await seedCaptureAttention(fixture);
    await captureScreens(page,fixture,agent,resourceAgent);
    await captureFleetSize(page,fixture,10);
    await captureFleetSize(page,fixture,60);
    if (errors.length) throw new Error(`Browser failures: ${errors.join('; ')}`);
    console.log('Live acceptance passed. Screenshots: test-results/live/screenshots/');
  } finally {await browser.close();}
} finally {await fixture.close();}
