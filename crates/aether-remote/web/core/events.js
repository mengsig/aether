import { object, string, integer, event as validateEvent } from './validate.js';
import { transcriptEntry } from './normalize.js';
import { actorLabel } from './actors.js';
/** @typedef {import('../types.js').State} State */
/** @param {State} state @param {unknown} value @returns {Partial<State>|null} */
export function applyEvent(state, value) {
  const event = validateEvent(value); const seq = Number(event.seq);
  if (seq <= state.lastSeq) return null;
  const kind = object(event.kind); const type = string(kind.type);
  const agentId = event.agent_id == null ? '' : string(event.agent_id);
  const name = type === 'custom' ? string(kind.name) : type;
  const detail = eventDetail(kind,name,event.actor,state.agents);
  /** @type {Partial<State>} */ const patch = {lastSeq:seq,activity:[...state.activity,{seq,at:string(event.at),agentId,kind:name,detail}].slice(-500)};
  if (name === 'submissions.evicted') {
    const payload = object(kind.payload);
    if (!integer(payload.n) || !integer(payload.max_submissions)) throw new Error('Invalid submission eviction report');
    patch.outbox = state.outbox.map(item => item.command.idempotency_key && ['pending','unknown'].includes(item.status)
      ? {...item,retentionRisk:'The daemon discarded saved delivery receipts. Check current state before retrying; the original key may no longer prevent duplicate work.'} : item);
  }
  if (type === 'question.asked') {
    const question = string(kind.question);
    patch.attention = [...state.attention.filter(item => item.kind !== 'question' || item.agentId !== agentId),
      {id:`question-${seq}`,kind:/** @type {const} */ ('question'),agentId,title:question,detail:question,at:string(event.at),questionSeq:seq,raw:event}].slice(-500);
  }
  if (type === 'message.sent' && kind.from && object(kind.from).actor === 'user') {
    patch.attention = state.attention.filter(item => item.kind !== 'question' || item.agentId !== kind.to);
  }
  if (['agent.completed','agent.cancelled','agent.failed'].includes(type)) {
    patch.attention = state.attention.filter(item => item.kind !== 'question' || item.agentId !== agentId);
  }
  if (type === 'approval.resolved') patch.attention = state.attention.filter(item => item.id !== kind.id && item.id !== kind.approval);
  if (type === 'agent.message') {
    const entry = transcriptEntry({seq,at:event.at,invocation:kind.invocation,message:{role:kind.role,content:kind.content}});
    if (Object.hasOwn(state.transcripts,agentId)) {
      const entries = [...state.transcripts[agentId].filter(item => item.seq !== seq),entry].sort((a,b)=>a.seq-b.seq);
      patch.transcripts = {...state.transcripts,[agentId]:entries};
    }
    patch.streams = {...state.streams}; delete patch.streams[agentId];
  }
  return patch;
}
/** @param {Record<string,unknown>} kind @param {string} name @param {unknown} actor @param {State['agents']} agents */
function eventDetail(kind,name,actor,agents) {
  if (name === 'submissions.evicted') return 'Saved delivery receipts were discarded. Review unresolved deliveries before retrying.';
  if (name === 'remote.restricted') return 'Remote administration changed. Details are available only on the host.';
  if (name === 'approval.resolved') return `${String(kind.decision)} · Decided by: ${actorLabel(actor,agents)}`;
  return String(kind.question ?? kind.summary ?? kind.error ?? kind.reason ?? name);
}
/** @param {State} state @param {Record<string,unknown>} frame @returns {Partial<State>} */
export function applyDelta(state, frame) {
  const agentId = string(frame.agent); const invocation = string(frame.invocation); const delta = object(frame.delta);
  const streams = {...state.streams};
  if (delta.delta === 'turn_finished') { delete streams[agentId]; return {streams}; }
  const previous = streams[agentId];
  const stream = previous?.invocation === invocation ? {...previous} : {agentId,invocation,text:'',thinking:''};
  if (delta.delta === 'text' || delta.delta === 'thinking') {
    const field = delta.delta === 'text' ? 'text' : 'thinking';
    if (typeof delta.text !== 'string') throw new Error('Invalid stream text');
    const text = stream[field] + delta.text;
    if (text.length > 2_000_000) throw new Error('Live stream exceeds the display limit; reload its full transcript');
    stream[field] = text;
  }
  if (Object.keys(streams).length >= 100 && !streams[agentId]) throw new Error('Too many concurrent streams');
  streams[agentId] = stream; return {streams};
}
