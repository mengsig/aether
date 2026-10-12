import {test,expect} from '@playwright/test';

/** @param {import('@playwright/test').Page} page @param {string} [recovery] @param {boolean} [unconfirmed] */
async function mount(page,recovery='unknown',unconfirmed=false) {
  await page.goto('/#pair');
  await page.evaluate(async ({recovery,unconfirmed})=>{
    const path='/ui/delivery-actions.js';const {deliveryActions}=await import(path);
    const storePath='/core/store.js';const {createStore}=await import(storePath);
    const feedbackPath='/ui/feedback.js';const {createConfirm}=await import(feedbackPath);
    const transport={onFrame:()=>()=>{},onConnection:()=>()=>{}};
    const store=createStore(transport,{storage:null});
    const item={id:'original-key',agentId:'agt_00000000000000000000000001',text:'Original message',status:'unknown',horizon:'first-horizon',recovery,
      ...(unconfirmed?{unconfirmed:{reason:'horizon_ahead',message:'The daemon cannot confirm this delivery.'}}:{}),
      command:{cmd:'send_message',to:'agt_00000000000000000000000001',content:'Original message',idempotency_key:'original-key'}};
    store.patch({connection:'online',role:'operator',outbox:[item]});store.refresh=async()=>{};
    const root=document.createElement('main');document.body.replaceChildren(root);
    /** @type {import('../types.js').Command[]} */ const commands=[];
    const context={store,confirm:createConfirm(root),command:async (/** @type {import('../types.js').Command} */ command)=>{commands.push(command);return {reply:'ok'};}};
    root.append(deliveryActions(item,context));
    Reflect.set(window,'deliveryTest',{store,commands,item});
  },{recovery,unconfirmed});
}

test('unconfirmed deliveries require a decision and preserve the original key',async({page})=>{
  await mount(page);
  await expect(page.getByRole('button',{name:'Mark delivered…'})).toBeVisible();
  expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').commands)).toEqual([]);
  await page.getByRole('button',{name:'Send again…'}).click();
  await page.getByRole('button',{name:'Keep as is'}).click();
  expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').commands)).toEqual([]);
  await page.getByRole('button',{name:'Send again…'}).click();
  await page.getByRole('button',{name:'Confirm',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>Reflect.get(window,'deliveryTest').commands)).toEqual([
    {cmd:'send_message',to:'agt_00000000000000000000000001',content:'Original message',idempotency_key:'original-key'},
  ]);
});

test('automatic lookup hides decisions and changes during a confirmation prevent resend',async({page})=>{
  await mount(page,'resolving');
  await expect(page.getByRole('button')).toHaveCount(0);
  await mount(page);
  await page.getByRole('button',{name:'Send again…'}).click();
  await page.evaluate(()=>{
    const {store,item}=Reflect.get(window,'deliveryTest');
    store.patch({outbox:[{...item,recovery:'resolving'}]});
  });
  await page.getByRole('button',{name:'Confirm',exact:true}).click();
  await expect(page.getByRole('status')).toContainText('Delivery status changed');
  expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').commands)).toEqual([]);
});

test('permanently unconfirmed deliveries keep their choices during further lookups',async({page})=>{
  await mount(page,'resolving',true);
  for (const name of ['Send again…','Mark delivered…','Discard…']) await expect(page.getByRole('button',{name})).toBeVisible();
  expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').commands)).toEqual([]);
  await page.getByRole('button',{name:'Send again…'}).click();
  await page.getByRole('button',{name:'Keep as is'}).click();
  expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').commands)).toEqual([]);
});

test('marking delivered and discarding only remove local records',async({page})=>{
  for (const action of ['Mark delivered…','Discard…']) {
    await mount(page);
    await page.evaluate(()=>{
      const state=Reflect.get(window,'deliveryTest');state.decisions=[];
      const dismiss=state.store.dismissReceipt.bind(state.store);
      state.store.dismissReceipt=(/** @type {string} */ id,/** @type {string} */ decision)=>{state.decisions.push(decision);return dismiss(id,decision);};
    });
    await page.getByRole('button',{name:action}).click();
    await page.getByRole('button',{name:'Confirm',exact:true}).click();
    await expect.poll(()=>page.evaluate(()=>Reflect.get(window,'deliveryTest').store.get().outbox)).toEqual([]);
    expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').commands)).toEqual([]);
    expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').decisions)).toEqual([action==='Mark delivered…'?'delivered':'discarded']);
  }
});

test('a rejected asynchronous delivery decision remains visible and keeps the receipt',async({page})=>{
  await mount(page);
  await page.evaluate(()=>{
    const {store}=Reflect.get(window,'deliveryTest');
    store.dismissReceipt=async()=>{throw new Error('Delivery changed while waiting for another tab. Review its latest status.');};
  });
  await page.getByRole('button',{name:'Mark delivered…'}).click();
  await page.getByRole('button',{name:'Confirm',exact:true}).click();
  await expect(page.getByRole('status')).toContainText('Delivery changed while waiting for another tab');
  expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').store.get().outbox)).toHaveLength(1);
  expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').commands)).toEqual([]);
});

test('successful inline retry clears failure feedback without restoring the sent draft',async({page})=>{
  await mount(page);
  await page.evaluate(async()=>{
    const composerPath='/features/conversation/composer.js';const {composer}=await import(composerPath);
    const tailPath='/features/conversation/tail.js';const {transcriptTail}=await import(tailPath);
    const feedbackPath='/ui/feedback.js';const {createConfirm}=await import(feedbackPath);
    const {store,commands}=Reflect.get(window,'deliveryTest');
    const agent='agt_00000000000000000000000001';
    store.patch({ready:true,agents:[{id:agent,name:'Review',status:'waiting'}],outbox:[]});
    const root=document.createElement('main');document.body.replaceChildren(root);
    const context={store,confirm:createConfirm(root),navigate:()=>{},toast:()=>{},command:async (/** @type {import('../types.js').Command} */ command)=>{
      commands.push(command);
      const failed=commands.length===1;
      store.patch({outbox:[{id:command.idempotency_key,agentId:agent,text:command.content,command,status:failed?'failed':'sent'}]});
      if (failed) throw new Error('temporarily unavailable');
      return {reply:'message_sent',data:{message:'msg_00000000000000000000000001'}};
    }};
    const compose=composer(context,agent,()=>{});const tail=transcriptTail(root,context,agent);
    root.append(compose.root);store.subscribe(()=>{compose.update();tail.update(store.get());});
  });
  await page.getByRole('textbox',{name:'Message draft'}).fill('Original draft');
  await page.getByRole('button',{name:'Send',exact:true}).click();
  await expect(page.locator('.conversation-delivery-failed')).toBeVisible();
  await expect(page.locator('.conversation-compose-status')).not.toContainText('Message not sent');
  await page.getByRole('button',{name:'Send again…'}).click();
  await page.getByRole('button',{name:'Confirm',exact:true}).click();
  await expect(page.locator('.conversation-delivery-failed')).toHaveCount(0);
  await expect(page.getByRole('textbox',{name:'Message draft'})).toHaveValue('');
  await expect(page.locator('.conversation-compose-status')).not.toContainText('Message not sent');
  const commands=await page.evaluate(()=>Reflect.get(window,'deliveryTest').commands);
  expect(commands).toHaveLength(2);expect(commands[1]).toEqual(commands[0]);
});

test('failed draft restoration keeps the text and explains that it is not saved',async({page})=>{
  await mount(page);
  await page.evaluate(async()=>{
    const composerPath='/features/conversation/composer.js';const {composer}=await import(composerPath);
    const {store}=Reflect.get(window,'deliveryTest');const agent='agt_00000000000000000000000001';
    store.patch({ready:true,agents:[{id:agent,name:'Review',status:'waiting'}],outbox:[]});
    const root=document.createElement('main');document.body.replaceChildren(root);
    const context={store,navigate:()=>{},toast:()=>{},command:async()=>{throw new Error('Receipt storage unavailable');}};
    root.append(composer(context,agent,()=>{}).root);
    const save=store.setDraft.bind(store);let cleared=false;
    store.setDraft=(/** @type {string} */ id,/** @type {string} */ text)=>{
      if (cleared) throw new Error('Draft storage unavailable');
      save(id,text);if (text==='') cleared=true;
    };
  });
  await page.getByRole('textbox',{name:'Message draft'}).fill('Keep this draft');
  await page.getByRole('button',{name:'Send',exact:true}).click();
  await expect(page.getByRole('textbox',{name:'Message draft'})).toHaveValue('Keep this draft');
  await expect(page.locator('.conversation-compose-status')).toContainText('Message not sent: Receipt storage unavailable');
  await expect(page.locator('.conversation-compose-status')).toContainText('could not be saved on this device');
  expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').store.getDraft('agt_00000000000000000000000001'))).toBe('');
  expect(await page.evaluate(()=>Reflect.get(window,'deliveryTest').store.get().outbox)).toEqual([]);
});
