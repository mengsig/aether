import { array, integer, object, string } from './validate.js';
/** @param {unknown} value @returns {import('../types.js').Agent} */
export function agent(value) {
  const raw = object(value, 'agent');
  const config = object(raw.config); const scope = object(raw.scope); const model = object(raw.model);
  const usage = raw.stats && object(raw.stats).usage ? object(object(raw.stats).usage) : null;
  const input = usage?.input_tokens; const output = usage?.output_tokens;
  const tokens = typeof input === 'number' && typeof output === 'number' ? integer(input) + integer(output) : null;
  const cost = typeof usage?.cost_usd === 'number' && Number.isFinite(usage.cost_usd) && usage.cost_usd >= 0 ? usage.cost_usd : null;
  return {id:string(raw.id),name:string(raw.name),parent:raw.parent == null ? null : string(raw.parent),
    role:string(raw.role),status:string(object(raw.status).state),paused:raw.paused === true,
    model:`${string(model.provider)}/${string(model.model)}`,effort:typeof config.effort === 'string' ? config.effort : 'default',
    workspace:typeof scope.workspace_root === 'string' ? scope.workspace_root : '',createdAt:string(raw.created_at),updatedAt:string(raw.updated_at),
    run:string(raw.run),tokens,cost,costComplete:usage?.cost_complete === true,
    approval:config.approval ? array(object(config.approval).require_for).map(value => string(value)).join(', ') || 'unrestricted' : 'unknown',raw};
}
/** @param {unknown} value @returns {import('../types.js').Attention} */
export function approval(value) {
  const raw = object(value, 'approval');
  return {id:string(raw.id),kind:'approval',agentId:string(raw.agent_id),title:string(raw.tool),
    detail:string(raw.description),at:string(raw.requested_at),raw};
}
/** @param {unknown} value @returns {import('../types.js').TranscriptEntry} */
export function transcriptEntry(value) {
  const item = object(value, 'transcript'); const message = object(item.message, 'transcript message');
  const content = array(message.content).map(block => {
    const validated = object(block, 'content block'); string(validated.type, 'content block type'); return validated;
  });
  return {seq:integer(item.seq),at:string(item.at),invocation:item.invocation == null ? undefined : string(item.invocation),
    message:{role:string(message.role),content}};
}
/** @template T @param {T[]} values @param {(value:T)=>string|number} key */
export function unique(values, key) {
  if (new Set(values.map(key)).size !== values.length) throw new Error('Duplicate records in daemon response');
  return values;
}
/** @param {unknown} value @returns {import('../types.js').Attention} */
export function question(value) {
  const raw = object(value); const seq = integer(raw.question_seq);
  if (!seq) throw new Error('Question sequence must be positive');
  return {id:`question-${seq}`,kind:'question',agentId:string(raw.agent),title:string(raw.question),detail:string(raw.question),at:string(raw.asked_at),questionSeq:seq,raw};
}
/** @param {unknown} value @returns {import('../types.js').Attention} */
export function agentAttention(value) {
  const raw = object(value); const kind = raw.kind; const agentId = string(raw.agent);
  if (kind !== 'agent_failed' && kind !== 'agent_blocked') throw new Error('Invalid agent attention kind');
  return {id:`attention-${kind}-${agentId}`,kind,agentId,title:kind === 'agent_failed' ? 'Agent failed' : 'Agent blocked',detail:string(raw.reason),at:string(raw.since),raw};
}
