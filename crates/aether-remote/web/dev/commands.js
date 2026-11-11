import { createFixtures, models, mockId } from './fixtures.js';
import { integer, object, string } from '../core/validate.js';
/** @typedef {ReturnType<typeof createFixtures>} Fixtures */
/** @typedef {import('../types.js').Command} Command */
/** @typedef {import('../types.js').Reply} Reply */
/** @param {Fixtures} state @param {Command} command @returns {Reply|null} */
export function readCommand(state, command) {
  const lookup = new Map([
    ['list_tasks',['tasks',state.tasks]],['list_agents',['agents',state.agents]],['list_approvals',['approvals',state.approvals]],['list_models',['models',models]],
    ['memories',['memories',state.memories]],['list_artifacts',['artifacts',state.artifacts]],['list_schedules',['schedules',state.schedules]],
    ['list_approval_rules',['approval_rules',state.rules]],['schedule_history',['schedule_history',[]]],
    ['mcp_servers',['mcp_servers',[{name:'aether',state:'ready',transport:'stdio',tools:[]}]]],
    ['providers',['providers',[{name:'Codex',provider:'codex',configured:true,enabled:true,engine:true,models:1,open_model_list:false,protocol:'native',health:{provider:'codex',running:2}}]]],
    ['auth_status',['auth',[{provider:'codex',provider_name:'Codex',configured:true,subscription:true,source:'local session'}]]],
    ['list_runs',['runs',[{id:'run_00000000000000000000000001',run:'run_00000000000000000000000001',started_at:state.agents[0].created_at,updated_at:state.agents[0].updated_at,agents:state.agents.length,agents_live:5,usage:usageSummary(state)}]]],
    ['get_run_report',['run_report',{run:'run_00000000000000000000000001',schema:1,from_seq:1,to_seq:state.seq,generated_at:new Date().toISOString(),body:{outcome:'running',objective:'Remote dashboard release',started_at:'2026-10-12T08:30:00Z',duration_ms:120000,events:state.seq,usage:state.agents[0].stats.usage,agents:[],approvals:[],artifacts:[],failures:[],open_loops:[],tasks:[],timeline:[],tools:[]}}]],
  ]);
  if (command.cmd === 'get_inbox') return {reply:'inbox',data:inboxFixtures(state,command)};
  if (command.cmd === 'get_attention') return attentionReply(state);
  const found = lookup.get(command.cmd);
  if (found) return {reply:String(found[0]),data:found[1]};
  if (command.cmd === 'get_task') { const task=state.tasks.find(item=>item.id===command.id); if (!task) throw new Error('Task not found'); return {reply:'task',data:task}; }
  if (command.cmd === 'get_agent') return {reply:'agent',data:requireAgent(state,command.id)};
  if (command.cmd === 'get_transcript') return {reply:'transcript',data:(state.transcripts.get(string(command.agent)) ?? []).filter(entry => entry.seq > Number(command.after ?? 0))};
  if (command.cmd === 'get_events') return {reply:'events',data:state.events.filter(event => event.seq > Number(command.after ?? 0)).slice(0,Number(command.limit ?? 100))};
  if (command.cmd === 'get_tree') return {reply:'tree',data:{roots:state.agents.filter(agent=>!agent.parent).map(agent=>agent.id),nodes:Object.fromEntries(state.agents.map(agent => [agent.id,{agent,children:state.agents.filter(child => child.parent === agent.id).map(child => child.id),subtree_usage:agent.stats?.usage ?? usageSummary({agents:[]})}]))}};
  if (command.cmd === 'status') return {reply:'status',data:{instance_id:'mock-daemon-aether',version:'mock-1.0.1',pid:process.pid,started_at:state.agents[0].created_at,last_seq:state.seq,agents_live:5,agents_running:2,agents_blocked:1,agents_waiting:1,tasks_active:3,pending_approvals:state.approvals.length,usage_total:usageSummary(state)}};
  if (command.cmd === 'ping') return {reply:'ok'};
  if (command.cmd === 'get_artifact') return {reply:'artifact',data:state.artifacts.find(item => item.id === command.id)};
  if (command.cmd === 'read_artifact') return {reply:'artifact_bytes',data:{artifact:state.artifacts[0],base64:Buffer.from('--- a/transport.js\n+++ b/transport.js\n+validateFrame(frame);\n').toString('base64'),truncated:false}};
  return workspaceRead(command);
}
/** @param {Command} command @returns {Reply|null} */
function workspaceRead(command) {
  if (command.cmd === 'list_workspaces') return {reply:'workspaces',data:[{path:'/work/aether',exists:true,agents:7,running:2,active_at:new Date().toISOString()},{path:'/work/notes',exists:true,agents:0,running:0,active_at:new Date().toISOString()}]};
  if (command.cmd === 'get_git_summary') return {reply:'git_summary',data:{repository:true,root:'/work/aether',branch:'remote-dashboard',dirty:true,staged:1,modified:2,untracked:0,ahead:1,behind:0}};
  if (command.cmd === 'get_push_config') return {reply:'push_config',data:{enabled:false,vapid_public_key:null,subscribed:false,previews:false,kinds:[]}};
  if (command.cmd === 'validate_workspace') return {reply:'workspace_validation',data:{valid:true,path:command.path,git:{repository:true,root:'/work/aether',branch:'remote-dashboard',dirty:true,staged:1,modified:2,untracked:0,ahead:1,behind:0}}};
  if (command.cmd !== 'complete_path') return null;
  const root = command.kind === 'workspace_file' ? String(command.workspace) : String(command.prefix || '/work');
  if (!root.startsWith('/work') || root.includes('..')) throw new Error('Path is outside the mock workspace');
  const files = ['src','docs','README.md','package.json'].map(name => ({name,path:`${root}/${name}`,is_dir:!name.includes('.')}));
  return {reply:'path_completions',data:{entries:files.filter(file => command.kind !== 'directory' || file.is_dir).map(file => ({path:command.kind === 'workspace_file' ? file.name : file.path,is_dir:file.is_dir})),truncated:false}};
}
/** @param {Fixtures} state @param {unknown} id */
function requireAgent(state, id) {
  string(id,'agent ID'); const agent = state.agents.find(item => item.id === id);
  if (!agent) throw new Error('Agent not found'); return agent;
}
/** @param {Fixtures} state @param {Command} command @returns {Reply} */
export function mutateCommand(state, command) {
  if (['send_message','reply_to_question'].includes(command.cmd)) {
    const agent = requireAgent(state,command.to); const text = string(command.content,'message content');
    if (command.cmd === 'reply_to_question' && !state.events.some(event => event.seq === command.question_seq && event.kind.type === 'question.asked')) throw new Error('Question is stale');
    const entries = state.transcripts.get(agent.id);
    if (!entries) throw new Error('Transcript is missing');
    entries.push({seq:++state.seq,at:new Date().toISOString(),invocation:'inv_00000000000000000000000002',message:{role:'user',content:[{type:'text',text}]}});
    agent.status = {state:'running'}; return {reply:'message_sent',data:{message:mockId('msg',state.seq)}};
  }
  if (command.cmd === 'resolve_approval') {
    const index = state.approvals.findIndex(item => item.id === command.id);
    if (index < 0) throw new Error('Approval is no longer pending');
    if (!['approved','denied'].includes(String(command.decision))) throw new Error('Invalid approval decision');
    state.approvals.splice(index,1); return {reply:'ok'};
  }
  if (['pause','resume','interrupt','cancel','wake','update_agent','set_approvals','compact'].includes(command.cmd)) {
    const agent = requireAgent(state,command.agent ?? command.id);
    if (command.cmd === 'pause' || command.cmd === 'resume') agent.paused = command.cmd === 'pause';
    if (command.cmd === 'interrupt') agent.status = {state:'ready'};
    if (command.cmd === 'cancel') agent.status = {state:'cancelled'};
    if (command.cmd === 'wake') agent.status = {state:'running'};
    if (command.cmd === 'update_agent') updateAgent(agent,object(command.patch));
    if (command.cmd === 'set_approvals') agent.config.approval = /** @type {{require_for:string[]}} */ (object(command.policy));
    return command.cmd === 'pause' ? {reply:'paused',data:{agents:[agent],mid_turn:agent.status.state === 'running' ? [agent.id] : []}} : {reply:'ok'};
  }
  const created = createResource(state,command);
  return created ?? resourceMutation(state,command);
}
/** @param {Fixtures} state @param {Command} command @returns {Reply} */
function resourceMutation(state, command) {
  if (command.cmd === 'pin_memory' || command.cmd === 'forget_memory') {
    const memory = state.memories.find(item => item.id === command.id);
    if (!memory) throw new Error('Memory not found');
    if (command.cmd === 'pin_memory') memory.pinned = command.pinned === true;
    else state.memories = state.memories.filter(item => item.id !== command.id);
    return {reply:'ok'};
  }
  if (command.cmd === 'create_memory') {
    const spec = object(command.spec); const memory = {...state.memories[0],id:mockId('mem',++state.seq),text:string(spec.text),kind:String(spec.kind ?? 'fact'),pinned:false};
    state.memories.push(memory); return {reply:'memory',data:memory};
  }
  if (command.cmd === 'update_schedule') {
    const schedule = state.schedules.find(item => item.id === command.id);
    if (!schedule) throw new Error('Schedule not found');
    schedule.status = {...schedule.status,state:command.action === 'pause' ? 'paused' : command.action === 'cancel' ? 'cancelled' : 'active'}; return {reply:'ok'};
  }
  if (command.cmd === 'revoke_approval_rule') {
    const rule = state.rules.find(item => item.id === command.id);
    if (!rule) throw new Error('Approval rule not found'); rule.revoked = {at:new Date().toISOString(),by:{actor:'user'}}; return {reply:'ok'};
  }
  if (command.cmd === 'archive') {
    const agent = requireAgent(state,object(command.target).agent);
    Object.assign(agent,{archived:command.archived === true}); return {reply:'ok'};
  }
  if (['subscribe_push','unsubscribe_push','test_push'].includes(command.cmd)) throw new Error('Push is unavailable in the mock daemon');
  throw new Error(`Unsupported mock command: ${command.cmd}`);
}

/** @param {Fixtures['agents'][number]} agent @param {Record<string,unknown>} patch */
function updateAgent(agent, patch) {
  if (patch.name !== undefined) agent.name = string(patch.name);
  if (patch.model !== undefined) {
    const model = object(patch.model); agent.model = {provider:string(model.provider),model:string(model.model)};
  }
  if (patch.effort !== undefined) agent.config.effort = string(patch.effort);
  if (patch.instructions !== undefined) Object.assign(agent.config,{instructions:string(patch.instructions)});
}
/** @param {Fixtures} state */
function nextAgentId(state) {
  const existing = new Set(state.agents.map(agent=>string(agent.id)));
  let sequence = integer(state.seq,'mock sequence'); let id;
  do {
    if (sequence >= Number.MAX_SAFE_INTEGER) throw new Error('Mock agent sequence exhausted');
    id = mockId('agt',++sequence);
  } while (existing.has(id));
  state.seq = sequence; return id;
}
/** @param {Fixtures} state @param {Command} command @returns {Reply|null} */
function createResource(state, command) {
  if (command.cmd === 'create_task') return createTask(state,command);
  if (command.cmd === 'create_agent' || command.cmd === 'restart') {
    const base = command.cmd === 'restart' ? requireAgent(state,command.agent) : state.agents[0];
    const agent = structuredClone(base); agent.id = nextAgentId(state); agent.parent = null;
    agent.role = typeof command.role === 'string' ? command.role : base.role;
    if (command.approval) agent.config.approval = /** @type {{require_for:string[]}} */ (object(command.approval));
    agent.name = typeof command.name === 'string' && command.name ? command.name : 'New agent';
    agent.status = {state:command.start === false ? 'created' : 'running'};
    if (command.scope) Object.assign(agent.scope,object(command.scope));
    if (command.model) {const model=object(command.model);agent.model={provider:string(model.provider),model:string(model.model)};}
    if (command.config) Object.assign(agent.config,object(command.config));
    state.agents.push(agent); state.transcripts.set(agent.id,[]); return {reply:'agent',data:agent};
  }
  if (command.cmd === 'create_schedule') return createSchedule(state,command);
  if (command.cmd === 'add_approval_rule') {
    const rule = structuredClone(state.rules[0]); rule.id = mockId('rule',++state.seq);
    if (!['approved','denied'].includes(String(command.decision))) throw new Error('Invalid approval decision');
    rule.scope = /** @type {typeof rule.scope} */ (object(command.scope)); rule.decision = String(command.decision);
    rule.match = /** @type {typeof rule.match} */ (object(command.match)); rule.revoked = null;
    state.rules.push(rule); return {reply:'approval_rule',data:rule};
  }
  return null;
}

/** @param {Fixtures} state @returns {Reply} */
function attentionReply(state) {
  const pending = new Map();
  for (const event of state.events) {
    const kind = /** @type {Record<string,unknown>} */ (event.kind);
    if (kind.type === 'question.asked') pending.set(event.agent_id,{agent:event.agent_id,question_seq:event.seq,question:kind.question,asked_at:event.at});
    if (kind.type === 'message.sent' && object(kind.from).actor === 'user') pending.delete(String(kind.to));
  }
  return {reply:'attention',data:{last_seq:state.seq,approvals:state.approvals,questions:[...pending.values()],
    agents:state.agents.filter(agent=>['failed','blocked'].includes(agent.status.state)).map(agent=>({agent:agent.id,agent_name:agent.name,kind:`agent_${agent.status.state}`,reason:agent.status.state === 'failed' ? 'CI provider returned a temporary error' : 'Waiting for browser access',since:agent.updated_at}))}};
}

/** @param {{agents:Fixtures['agents']}} state */
function usageSummary(state) {
  const total = {input_tokens:0,output_tokens:0,cache_read_tokens:0,cache_write_tokens:0,cost_usd:0,cost_complete:state.agents.length>0};
  for (const agent of state.agents) {
    const usage = agent.stats?.usage;
    if (!usage) { total.cost_complete = false; continue; }
    total.input_tokens += usage.input_tokens; total.output_tokens += usage.output_tokens;
    total.cache_read_tokens += usage.cache_read_tokens; total.cache_write_tokens += usage.cache_write_tokens;
    total.cost_usd += usage.cost_usd; total.cost_complete &&= usage.cost_complete;
  }
  return total;
}

/** @param {Fixtures} state @param {Command} command @returns {Reply} */
function createTask(state, command) {
  const objective = string(command.objective,'task objective');
  const owner = command.owner == null ? null : requireAgent(state,command.owner);
  const at = new Date().toISOString();
  const task = {id:mockId('tsk',++state.seq),objective,owner:owner?.id ?? null,creator:{actor:'user'},
    run:owner?.run ?? state.agents[0].run,status:{state:owner ? 'assigned' : 'queued'},priority:command.priority ?? 'normal',
    dependencies:command.dependencies ?? [],parent_task:command.parent_task ?? null,artifacts:[],created_at:at,updated_at:at};
  state.tasks.push(task); return {reply:'task',data:task};
}
/** @param {Fixtures} state @param {Command} command @returns {Reply} */
function createSchedule(state, command) {
  const spec = object(command.spec); const name = string(spec.name);
  if ((spec.target == null) === (spec.spawn == null)) throw new Error('A schedule needs exactly one target or spawn');
  const target = spec.target == null ? null : requireAgent(state,spec.target);
  const at = new Date().toISOString(); const recurrence = object(spec.recurrence);
  const due = typeof recurrence.at === 'string' ? recurrence.at : new Date(Date.now()+Number(recurrence.seconds ?? 60)*1000).toISOString();
  const schedule = {id:mockId('sch',++state.seq),name,target:target?.id ?? null,spawn:spec.spawn ?? null,run:target?.run ?? null,
    owner:{actor:'user'},created_at:at,updated_at:at,recurrence,missed:spec.missed ?? 'coalesce',busy:spec.busy ?? 'queue_one',
    payload:spec.payload ?? null,max_deliveries:spec.max_deliveries ?? null,deliveries:0,status:{state:'active',next_due:due}};
  state.schedules.push(/** @type {Fixtures['schedules'][number]} */ (schedule)); return {reply:'schedule',data:schedule};
}

/** @param {Fixtures} state @param {Command} command */
function inboxFixtures(state,command) {
  const agent = requireAgent(state,command.agent);
  const item = {id:'ibx_00000000000000000000000001',agent_id:agent.id,seq:1,delivered_at:agent.created_at,
    kind:{kind:'approval_resolved',approval:'apr_00000000000000000000000002',decision:'approved',
      by:{actor:'user',name:'Alex',origin:{transport:'device',device:'dev_00000000000000000000000001',name:'Review phone',connection:'mock'}},note:'Reviewed the exact tool arguments.'}};
  return Number(command.after) < 1 ? [item] : [];
}
