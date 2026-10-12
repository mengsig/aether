/** @typedef {{id:string,name:string,target:string|null,spawn?:unknown,run?:string|null,status:{state:string,next_due:string},created_at:string,updated_at:string,owner:{actor:string},deliveries:number,missed:string,busy:string,payload:unknown,recurrence:Record<string,unknown>,max_deliveries?:unknown}} MockSchedule */
export const ids = {"root": "agt_00000000000000000000000001", "web": "agt_00000000000000000000000002", "api": "agt_00000000000000000000000003", "tests": "agt_00000000000000000000000004", "docs": "agt_00000000000000000000000005", "ci": "agt_00000000000000000000000006", "research": "agt_00000000000000000000000007"};
const at = new Date(Date.now()-45*60_000).toISOString();
/** @param {string} id @param {string} name @param {string|null} parent @param {string} status @param {string} role */
function agent(id, name, parent, status, role) {
  return {id,name,parent,role,status:{state:status},paused:false,run:'run_00000000000000000000000001',created_at:at,updated_at:at,
    model:{provider:'codex',model:'gpt-6-astra'},config:{effort:'high',approval:{require_for:['external']}},
    scope:{workspace_root:'/work/aether',allowed_paths:['/work/aether'],network:'allowed'},
    stats:{turns:8,tool_calls:14,usage:{input_tokens:12400,output_tokens:4200,cache_read_tokens:8200,cache_write_tokens:0,cost_usd:0.84,cost_complete:true}}};
}
/** @param {number} seq @param {string} role @param {Record<string,unknown>[]} content */
const entry = (seq,role,content) => ({seq,at,invocation:'inv_00000000000000000000000001',message:{role,content}});
export function createFixtures() {
  const agents = [agent('agt_00000000000000000000000001','Release conductor',null,'running','orchestrator'),
    agent('agt_00000000000000000000000002','Remote dashboard','agt_00000000000000000000000001','running','frontend'),agent('agt_00000000000000000000000003','Protocol review','agt_00000000000000000000000001','waiting','reviewer'),
    agent('agt_00000000000000000000000004','Browser checks','agt_00000000000000000000000002','blocked','tester'),agent('agt_00000000000000000000000005','Operator guide','agt_00000000000000000000000001','completed','writer'),
    agent('agt_00000000000000000000000006','CI investigation',null,'failed','debugger'),agent('agt_00000000000000000000000007','Next sprint',null,'ready','planner')];
  Object.assign(agents[2].status,{condition:{on:'inbox'}});
  Object.assign(agents[3].status,{reason:{block:'inspection',detail:'Waiting for browser access'}});
  Object.assign(agents[5].status,{error:'CI provider returned a temporary error'});
  agents[2].model = {provider:'anthropic',model:'claude-opus-4-6'};
  agents[5].stats.usage.cost_complete = false;
  for (const [index,item] of agents.entries()) {
    item.stats.usage.input_tokens += index*1347; item.stats.usage.output_tokens += index*419;
    item.stats.usage.cost_usd = Number((0.24+index*0.39).toFixed(2));
  }
  Object.assign(agents[6],{stats:undefined});
  const transcripts = new Map(agents.map(item => [item.id,[
    entry(1,'user',[{type:'text',text:'Build a remote dashboard with a clear fleet view and safe operator controls.'}]),
    entry(2,'assistant',[{type:'thinking',text:'I will inspect the protocol and preserve its permission boundaries.'},{type:'text',text:'The fleet is connected. I’m checking authentication, reconnects, and keyboard navigation.\n\n- Preserve request correlation\n- Keep uncertain sends visible\n- Verify observer permissions'}]),
    entry(3,'assistant',[{type:'tool_use',id:'call_00000000000000000000000002',name:'exec_command',input:{cmd:'git diff --stat'}}]),
    entry(4,'user',[{type:'tool_result',tool_use_id:'call_00000000000000000000000002',content:[{type:'text',text:' core/transport.js | 142 +++++++++++++++++\n core/store.js | 98 ++++++++++++'}],is_error:false}]),
    entry(5,'assistant',[{type:'text',text:'```diff\n--- a/core/transport.js\n+++ b/core/transport.js\n@@ -1,2 +1,3 @@\n const pending = new Map();\n+const cursor = 0;\n```\n\nSee [the protocol guide](https://example.com/protocol). Unknown costs remain unknown.'}]),
  ]]));
  const approvals = [{id:'apr_00000000000000000000000001',agent_id:'agt_00000000000000000000000002',call_id:'call_00000000000000000000000001',tool:'git_push',description:'Push the reviewed dashboard branch to origin',
    argument:'git push origin remote-dashboard',side_effect:'external',resources:['repo:/work/aether','network'],requested_at:at,status:{state:'pending'}}];
  return {agents,transcripts,approvals,tasks:/** @type {Record<string,unknown>[]} */ ([]),events:[{id:'evt_00000000000000000000000001',seq:6,at,agent_id:'agt_00000000000000000000000003',actor:{actor:'agent',id:'agt_00000000000000000000000003'},kind:{type:'question.asked',question:'Should observers see archived runs in the fleet?'}}],seq:6,
    memories:[{id:'mem_00000000000000000000000001',text:'Observer devices can inspect state but cannot issue mutations.',kind:'decision',scope:{scope:'global'},confidence:1,importance:0.8,provenance:{actor:{actor:'user'}},tags:['remote','security'],pinned:true,created_at:at,updated_at:at,status:'accepted'}],
    artifacts:[{id:'art_00000000000000000000000001',agent:'agt_00000000000000000000000002',agent_id:'agt_00000000000000000000000002',run:'run_00000000000000000000000001',kind:{kind:'diff'},name:'Remote transport changes',mime:'text/x-diff',content_type:'text/x-diff',size_bytes:182,created_by:{actor:'user'},created_at:at,summary:'Validated request correlation and reconnect behavior'}],
    schedules:/** @type {MockSchedule[]} */ ([{id:'sch_00000000000000000000000001',name:'Daily release check',target:'agt_00000000000000000000000001',status:{state:'active',next_due:'2026-10-13T09:00:00Z'},created_at:at,updated_at:at,owner:{actor:'user'},deliveries:0,missed:'coalesce',busy:'queue_one',payload:'Review release readiness',recurrence:{kind:'cron',expression:'0 9 * * *',timezone:'UTC'}}]),
    rules:[{id:'rule_00000000000000000000000001',scope:{scope:'everywhere'},decision:'approved',match:{tool:'read_file'},created_at:at,created_by:{actor:'user'},revoked:/** @type {Record<string,unknown>|null} */ (null),note:'Read-only project inspection'}]};
}
export const models = [{model:{provider:'codex',model:'gpt-6-astra'},name:'GPT-6 Astra',provider_name:'Codex',configured:true,is_default:true,capabilities:{context_window:200000,max_output_tokens:32000,effort:true,tools:true,vision:true}},
  {model:{provider:'anthropic',model:'claude-opus-4-6'},name:'Claude Opus',provider_name:'Anthropic',configured:true,is_default:false,capabilities:{context_window:200000,max_output_tokens:32000,effort:true,tools:true,vision:true}}];

/** @param {string} prefix @param {number} sequence */
export function mockId(prefix, sequence) {
  if (!Number.isSafeInteger(sequence) || sequence < 1) throw new Error('Invalid mock ID sequence');
  if (!/^[a-z]+$/.test(prefix)) throw new Error('Invalid mock ID prefix');
  return `${prefix}_${String(sequence).padStart(26,'0')}`;
}
