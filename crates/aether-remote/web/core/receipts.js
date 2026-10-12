import { object, string } from './validate.js';
/** @typedef {import('../types.js').Outbox} Outbox */
/** @param {Outbox[]} outbox @param {Record<string,unknown>} frame @returns {Outbox[]} */
export function applyReceipt(outbox, frame) {
  const command = /** @type {import('../types.js').Command} */ (object(frame.command)); string(command.cmd);
  const id = string(frame.id); const phase = string(frame.phase);
  const previous = outbox.find(item => item.id === id);
  if (phase !== 'pending') {
    if (!['sent','failed','unknown'].includes(phase) || !previous) throw new Error('Invalid mutation receipt transition');
    if (phase !== 'sent' && frame.sendId && previous.sendId && frame.sendId !== previous.sendId) return outbox;
    if (previous.status === 'dismissed') return outbox;
    if (phase === 'unknown' && ['sent','failed'].includes(previous.status)) return outbox;
    if (frame.late === true && previous.status === 'sent') return outbox;
    return outbox.map(item => item.id === id ? {...item,status:/** @type {'sent'|'failed'|'unknown'} */ (phase),error:typeof frame.error === 'string' ? frame.error : undefined,reply:phase === 'sent' && frame.reply ? /** @type {import('../types.js').Reply} */ (object(frame.reply)) : item.reply,recovery:undefined,retentionRisk:phase === 'sent' ? undefined : item.retentionRisk} : item);
  }
  if (previous?.status === 'pending' || previous?.status === 'sent' || previous?.status === 'dismissed') throw new Error('This command is already pending or accepted');
  if (previous && JSON.stringify(previous.command) !== JSON.stringify(command)) throw new Error('Retry must preserve the original command');
  const horizon = previous ? previous.horizon ?? null : frame.horizon == null ? null : string(frame.horizon,'submission horizon');
  const createdOrder = previous?.createdOrder ?? Math.max(0,...outbox.map(row=>row.createdOrder ?? 0))+1;
  if (!Number.isSafeInteger(createdOrder)) throw new Error('Receipt creation order exhausted');
  const sendId = frame.sendId == null ? undefined : string(frame.sendId,'wire send identity');
  const attemptId = previous?.status === 'unknown' ? previous.attemptId ?? previous.id : sendId ?? crypto.randomUUID();
  const remaining = outbox.filter(item => item.id !== id);
  if (remaining.length >= 100) {
    const settled = remaining.findIndex(item => item.status === 'sent' || item.status === 'failed' || item.status === 'dismissed');
    if (settled < 0) throw new Error('Outbox has 100 unresolved commands. Inspect them before sending more.');
    remaining.splice(settled,1);
  }
  const agentId = typeof command.to === 'string' ? command.to : typeof command.agent === 'string' ? command.agent : typeof command.id === 'string' ? command.id : '';
  return [...remaining,{id,agentId,text:typeof command.content === 'string' ? command.content : command.cmd,status:'pending',horizon,unconfirmed:previous?.unconfirmed,decision:previous?.unconfirmed && frame.automaticRecovery !== true ? 'send_again' : previous?.decision,attemptId,sendId,createdOrder,retentionRisk:previous?.retentionRisk,command}];
}
