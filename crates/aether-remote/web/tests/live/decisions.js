import {expect} from '@playwright/test';
import {until} from './host.js';

/** @typedef {import('@playwright/test').Page} Page */
/** @typedef {import('./browser.js').Fixture} Fixture */
/** @typedef {{id:string,agent_id:string,tool:string,side_effect:string,argument:string,status:{state:string}}} Approval */
/** @typedef {{agent:string,question_seq:number,question:string}} Question */

/** @param {Fixture} fixture @param {string} objective @param {string} name */
async function seedAgent(fixture,objective,name) {
  expect(fixture.root).toMatch(/^\/var\/tmp\/aw-/);
  expect(objective.length).toBeGreaterThan(0);
  const reply=await fixture.request({cmd:'create_agent',name,role:'chat',objective,
    model:{provider:'mock',model:'mock-1'},scope:{workspace_root:fixture.workspace,network:'allowed'},
    config:{long_lived:true,effort:'low'},approval:{require_for:['external']}});
  expect(reply.reply).toBe('agent');
  expect(reply.data.id).toMatch(/^agt_[0-9A-Z]{26}$/);
  return /** @type {string} */ (reply.data.id);
}

/** @param {Fixture} fixture @param {string} agent */
async function pendingApproval(fixture,agent) {
  const reply=await until(()=>fixture.request({cmd:'list_approvals'}),
    value=>value.data.some((/** @type {Approval} */ item)=>item.agent_id===agent),'genuine external approval');
  const approval=/** @type {Approval} */ (reply.data.find((/** @type {Approval} */ item)=>item.agent_id===agent));
  expect(approval.tool).toBe('web_fetch');
  expect(approval.side_effect).toBe('external');
  expect(approval.argument).toBe('https://example.com/changelog');
  return approval;
}

/** @param {Page} page @param {Fixture} fixture @param {'approved'|'denied'} decision */
async function checkApproval(page,fixture,decision) {
  const agent=await seedAgent(fixture,'Fetch the changelog page',`Live approval ${decision}`);
  const approval=await pendingApproval(fixture,agent);
  await page.goto(`${fixture.url}/#attention`);
  const card=page.locator(`[data-attention="${approval.id}"]`);
  await expect(card).toContainText('web_fetch');
  await expect(card).toContainText(approval.argument);
  const button=decision==='approved'?'Approve…':'Deny…';
  await card.getByRole('button',{name:button,exact:true}).click();
  const dialog=page.getByRole('dialog');
  await expect(dialog).toContainText(decision==='approved'?'Approve this exact tool call?':'Deny this tool call?');
  await expect(dialog).toContainText('Tool: web_fetch');
  await expect(dialog).toContainText('Side effect: external');
  await expect(dialog).toContainText('https://example.com/changelog');
  await dialog.getByRole('button',{name:'Keep as is',exact:true}).click();
  expect((await pendingApproval(fixture,agent)).id).toBe(approval.id);
  await card.getByRole('button',{name:button,exact:true}).click();
  await dialog.getByRole('button',{name:'Confirm',exact:true}).click();
  await until(()=>fixture.request({cmd:'list_approvals'}),
    value=>!value.data.some((/** @type {Approval} */ item)=>item.id===approval.id),'approval resolved');
  await expect(card).toHaveCount(0);
  await expect(fixture.request({cmd:'resolve_approval',id:approval.id,decision})).rejects.toThrow(/already resolved/);
  return agent;
}

/** @param {Page} page @param {Fixture} fixture */
async function checkQuestion(page,fixture) {
  const agent=await seedAgent(fixture,'live acceptance question','Live question');
  const attention=await until(()=>fixture.request({cmd:'get_attention'}),
    value=>value.data.questions.some((/** @type {Question} */ item)=>item.agent===agent),'ask_user question');
  const question=/** @type {Question} */ (attention.data.questions.find((/** @type {Question} */ item)=>item.agent===agent));
  expect(question.question_seq).toBeGreaterThan(0);
  expect(question.question).toBe('Which acceptance color should I use?');
  await page.goto(`${fixture.url}/#attention`);
  const card=page.locator('.fleet-attention-card').filter({has:page.getByRole('link',{name:'Live question',exact:true})});
  await expect(card).toContainText(question.question);
  await card.getByLabel('Your reply',{exact:true}).fill('Use blue for this acceptance check.');
  await card.getByRole('button',{name:'Send reply',exact:true}).click();
  await until(()=>fixture.request({cmd:'get_attention'}),
    value=>!value.data.questions.some((/** @type {Question} */ item)=>item.question_seq===question.question_seq),'question answered');
  await expect(card).toHaveCount(0);
  await expect(fixture.request({cmd:'reply_to_question',to:agent,question_seq:question.question_seq,content:'duplicate'})).rejects.toThrow(/(no longer pending|not the pending question)/);
  return agent;
}

/** @param {Page} page @param {Fixture} fixture */
export async function checkDecisions(page,fixture) {
  expect(fixture.socket).toMatch(/^\/var\/tmp\/aw-.*\/daemon\.sock$/);
  expect(new URL(fixture.url).hostname).toBe('127.0.0.1');
  const denied=await checkApproval(page,fixture,'denied');
  const approved=await checkApproval(page,fixture,'approved');
  const question=await checkQuestion(page,fixture);
  return {denied,approved,question};
}
