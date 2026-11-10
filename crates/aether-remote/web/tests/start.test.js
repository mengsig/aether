import {test} from 'node:test';
import assert from 'node:assert/strict';
import {startCommand,modelOptions} from '../features/start/models.js';
import {spawnScheduleCommand} from '../features/controls/spawn-schedule.js';
const base = {objective:'Review authentication',name:'Security',workspace:'/work/aether',model:'codex/gpt-6-astra',effort:'high',approval:'ask',task:false,owner:''};
test('new agent retains scope, model and approvals without splitting model slashes',() => {
  const command = startCommand({...base,model:'openrouter/openai/model'});
  assert.equal(command.cmd,'create_agent');
  assert.deepEqual(command.model,{provider:'openrouter',model:'openai/model'});
  assert.deepEqual(command.scope,{workspace_root:'/work/aether'});
  assert.deepEqual(command.approval,{require_for:['external']});
});
test('task requires an explicit owner and objective; workspace remains host absolute',() => {
  assert.throws(() => startCommand({...base,task:true}),/Choose an agent/);
  assert.throws(() => startCommand({...base,objective:' '}),/Describe/);
  assert.throws(() => startCommand({...base,workspace:'..'}),/absolute/);
  assert.equal(startCommand({...base,task:true,owner:'agt_abc'}).owner,'agt_abc');
});
test('model response validates identities at boundary',() => {
  assert.throws(() => modelOptions({}),/model list/);
  assert.throws(() => modelOptions([{}]),/identity/);
  assert.deepEqual(modelOptions([{model:{provider:'codex',model:'default'},configured:true,is_default:true}])[0],{id:'codex/default',label:'default',configured:true,default:true});
});
test('scheduled fresh runs retain explicit scope/model, bounded occurrences and no existing target',() => {
  const command = spawnScheduleCommand({workspace:'/work',name:'Daily audit',model:'codex/default',objective:'Inspect recent changes',seconds:'86400',limit:'7',effort:'medium'});
  const spec = /** @type {Record<string,unknown>} */ (command.spec);
  assert.equal(spec.target,undefined);
  assert.equal(spec.max_deliveries,7);
  assert.deepEqual(spec.recurrence,{kind:'interval',seconds:86400});
  assert.throws(() => spawnScheduleCommand({workspace:'../bad'}),/absolute/);
});
