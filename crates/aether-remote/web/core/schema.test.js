import test from 'node:test';
import {readFileSync} from 'node:fs';
import {KEYED_COMMANDS} from './contract.js';
import assert from 'node:assert/strict';
import { assertProtocol } from '../dev/validate.js';
import { createMockServer } from '../dev/server.js';
import { createFixtures, models } from '../dev/fixtures.js';
import { mutateCommand, readCommand } from '../dev/commands.js';
test('realistic mock fixtures validate against the generated daemon schema',()=>{
  const state=createFixtures();
  const records=[['Agent',state.agents],['ApprovalRequest',state.approvals],['MemoryRecord',state.memories],['Artifact',state.artifacts],['Schedule',state.schedules],['ApprovalRule',state.rules],['ModelInfo',models],['Event',state.events],['TranscriptEntry',[...state.transcripts.values()].flat()]];
  for(const [name,values] of records) for(const value of values) assert.doesNotThrow(()=>assertProtocol(name,value),`${name} ${value.id ?? value.seq ?? ''}`);
});
test('command schema rejects malformed approval policy and effort',()=>{
  assert.throws(()=>assertProtocol('ClientFrame',{req:1,cmd:'create_agent',role:'assistant',approval:'approved'}),/Invalid/);
  assert.throws(()=>assertProtocol('ClientFrame',{req:1,cmd:'create_agent',role:'assistant',config:{effort:'extreme'}}),/Invalid/);
  assert.doesNotThrow(()=>assertProtocol('ClientFrame',{req:1,cmd:'create_agent',role:'assistant',approval:{require_for:['external']},config:{effort:'high'}}));
});
test('mock approval resolution requires actual protocol decision names',()=>{
  assert.throws(()=>mutateCommand(createFixtures(),{cmd:'resolve_approval',id:'apr_00000000000000000000000001',decision:'allow'}),/decision/);
  assert.equal(mutateCommand(createFixtures(),{cmd:'resolve_approval',id:'apr_00000000000000000000000001',decision:'approved'}).reply,'ok');
});

test('message and question receipts expose the protocol message identity',()=>{
  const state=createFixtures(); const to=state.agents[0].id;
  const question=state.events.find(event=>event.kind.type==='question.asked');
  assert.ok(question);
  for(const command of [{cmd:'send_message',to,content:'Hello'},
    {cmd:'reply_to_question',to:question.agent_id,question_seq:question.seq,content:'Proceed'}]) {
    assertProtocol('ClientFrame',{req:1,...command});
    const reply=mutateCommand(state,command); assertProtocol('Reply',reply);
    assert.match(reply.data.message,/^msg_/);
  }
});

test('all existing mock read replies use the documented daemon result shapes',()=>{
  const state=createFixtures();
  const commands=['list_agents','list_approvals','list_models','memories','list_artifacts','list_schedules','list_approval_rules','schedule_history','mcp_servers','providers','auth_status','list_runs','get_run_report','get_agent','get_transcript','get_events','get_tree','status','ping','get_artifact','read_artifact'];
  for(const cmd of commands){
    const command={cmd,id:cmd.includes('artifact')?'art_00000000000000000000000001':state.agents[0].id,agent:state.agents[0].id,after:0};
    assert.doesNotThrow(()=>assertProtocol('Reply',readCommand(state,command)),cmd);
  }
});
test('mock task creation, lifecycle and schedule replies match protocol schemas',()=>{
  const state=createFixtures();const agent=state.agents[0].id;
  const commands=[{cmd:'create_task',objective:'Review the dashboard',owner:agent,priority:'high'},
    ...['pause','resume','interrupt','compact','wake','cancel'].map(cmd=>({cmd,agent})),
    {cmd:'create_schedule',spec:{name:'Fresh audit',spawn:{role:'reviewer',objective:'Check release'},recurrence:{kind:'interval',seconds:60},max_deliveries:7,busy:'skip',missed:'skip',payload:{review:true}}}];
  for(const command of commands){
    assertProtocol('ClientFrame',{req:1,...command});const reply=mutateCommand(state,command);assertProtocol('Reply',reply);
    if(command.cmd==='create_task') assert.equal(readCommand(state,{cmd:'get_task',id:reply.data.id}).data.objective,'Review the dashboard');
  }
  const schedule=state.schedules.at(-1);assert.equal(schedule.max_deliveries,7);assert.equal(schedule.busy,'skip');assert.equal(schedule.missed,'skip');
  assert.equal(schedule.target,null);assert.deepEqual(schedule.payload,{review:true});assert.equal(schedule.deliveries,0);
});
test('protocol 18 remote read replies validate against the final generated schema',()=>{
  const state=createFixtures();
  for(const cmd of ['get_attention','list_workspaces','get_git_summary','get_push_config','validate_workspace','complete_path']){
    const command={cmd,kind:'directory',prefix:'/work',path:'/work/aether',workspace:'/work/aether'};
    assertProtocol('ClientFrame',{req:1,...command});assertProtocol('Reply',readCommand(state,command));
  }
});

test('mock broadcast events include protocol message identity',async()=>{
  const server=createMockServer();
  try {
    server.broadcastMessage({cmd:'send_message',to:server.state.agents[0].id,content:'Hello'},'msg_00000000000000000000000001');
    const event=server.state.events.at(-1);assertProtocol('Event',event);assert.equal(event.kind.message,'msg_00000000000000000000000001');
  } finally {await server.close();}
});

test('every schema-declared idempotency key is sent by the browser',()=>{
  const definitions=JSON.parse(readFileSync(new URL('../dev/protocol.schema.json',import.meta.url))).$defs;
  const keyed=definitions.ClientFrame.oneOf.filter(variant=>{
    const properties={...(variant.$ref?definitions[variant.$ref.split('/').at(-1)].properties:{}),...variant.properties};
    return properties.idempotency_key;
  }).map(variant=>variant.properties.cmd.const);
  assert.equal(keyed.length,18);assert.deepEqual([...KEYED_COMMANDS].sort(),keyed.sort());
});
test('create and restart as the first mock mutation allocate distinct agent identities',()=>{
  for(const cmd of ['create_agent','restart']) {
    const state=createFixtures();const existing=new Set(state.agents.map(agent=>agent.id));
    const command=cmd==='create_agent'?{cmd,role:'assistant',name:'First new agent'}:{cmd,agent:state.agents[0].id};
    assertProtocol('ClientFrame',{req:1,...command});
    const reply=mutateCommand(state,command);assertProtocol('Reply',reply);
    assert.equal(existing.has(reply.data.id),false);assert.equal(new Set(state.agents.map(agent=>agent.id)).size,state.agents.length);
    assert.equal(state.transcripts.has(reply.data.id),true);assertProtocol('Reply',readCommand(state,{cmd:'list_agents'}));
  }
});
