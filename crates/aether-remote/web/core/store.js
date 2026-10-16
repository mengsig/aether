import { assertSubmissionLock, SUBMISSION_LOCK_WAIT, withSubmissionLock } from './submission-locks.js';
import { readUnconfirmed } from './unconfirmed.js';
import { recoverSubmissions } from './recovery.js';
import { array, errorText, integer, object, string } from './validate.js';
import { agent, agentAttention, approval, noteEntry, question, transcriptEntry, unique } from './normalize.js';
import { applyDelta, applyEvent } from './events.js';
import { applyReceipt } from './receipts.js';
import { retire } from './retirements.js';
import { clearLocalState, interruptedReceipts, dismissReceipt, mergeLedger } from './ledger.js';
import { createDrafts, restoreSnapshot, saveSnapshot } from './persistence.js';
/** @typedef {import('../types.js').State} State */
/** @typedef {import('../types.js').Transport} Transport */
const NOTE_PAGE = 1000;
const NOTE_PAGES = 50;
/** @returns {State} */
function initialState() {
  return {ready:false,connection:'offline',connectionDetail:'Not connected',role:'observer',agents:[],attention:[],activity:[],
    transcripts:{},streams:{},outbox:[],lastSeq:0,version:'',deviceName:'',daemonId:'',error:null};
}
export class RemoteStore {
  /** @param {Transport} transport @param {{storage?:Storage|null}} [options] */
  constructor(transport, options = {}) {
    this.transport = transport; this.state = initialState();
    /** @type {Set<()=>void>} */ this.listeners = new Set();
    /** @type {Promise<void>|null} */ this.refreshing = null;
    /** @type {ReturnType<typeof setTimeout>|undefined} */ this.refreshTimer = undefined;
    /** @type {Map<string,Promise<void>>} */ this.loading = new Map();
    /** @type {Set<string>} */ this.completeTranscripts = new Set();
    this.submissionLookup = false; this.recoveryEpoch = -1; this.submissionAbort = new AbortController(); this.disposed = false;
    /** @type {Set<string>} */ this.recoveryQueue = new Set();
    /** @type {Set<string>} */ this.deferredRecovery = new Set();
    /** @type {ReturnType<typeof setTimeout>|undefined} */ this.recoveryTimer = undefined;
    /** @type {Promise<void>|null} */ this.recovering = null;
    this.epoch = 0; this.refreshAgain = false; this.attentionSeq = 0; this.needsResync = false;
    /** @type {Record<string,unknown>[]|null} */ this.buffer = null;
    this.storage = options.storage === undefined ? globalThis.localStorage ?? null : options.storage;
    this.drafts = createDrafts(this.storage);
    const restored = restoreSnapshot(this.storage); this.deviceId = restored.deviceId ?? '';
    this.state = {...this.state,...restored};
    this.storageListener = (/** @type {Event} */ event) => this.externalStorage(/** @type {Event & {key?:string|null,newValue?:string|null}} */ (event));
    if ('document' in globalThis) globalThis.addEventListener('storage',this.storageListener);
    this.cleanup = [transport.onFrame(frame => this.receive(frame)),transport.onConnection((connection,detail) => this.connection(connection,detail))];
  }
  /** @param {Event & {key?:string|null,newValue?:string|null}} event */
  externalStorage(event) {
    if (event.key === 'aether.remote.state.v1' && event.newValue === null) {
      this.transport.disconnect(); this.state = initialState(); this.deviceId = ''; this.completeTranscripts.clear();
      for (const listener of this.listeners) listener(); return;
    }
    if ((!event.key?.startsWith('aether.remote.receipt.v1:') && !event.key?.startsWith('aether.remote.retired.v1:') && !event.key?.startsWith('aether.remote.unconfirmed.v1:')) || !this.state.daemonId) return;
    try {
      const previous = new Map(this.state.outbox.map(row=>[row.id,row.sendId]));
      this.state = {...this.state,outbox:mergeLedger(this.storage,this.state.daemonId,this.deviceId,this.state.outbox)};
      if (this.state.connection === 'online') for (const row of this.state.outbox) {
        if (!['pending','unknown'].includes(row.status) || previous.has(row.id) && previous.get(row.id) === row.sendId) continue;
        void this.resolvePendingSubmissions(row.id).catch(error=>this.patch({error:errorText(error)}));
      }
    }
    catch (error) { this.state = {...this.state,error:`Could not refresh delivery ledger: ${errorText(error)}`}; }
    for (const listener of this.listeners) listener();
  }
  get() { return this.state; }
  /** @param {()=>void} listener */
  subscribe(listener) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  /** @param {Partial<State>} patch @param {import('./submission-locks.js').SubmissionLock} [lease] */
  patch(patch,lease) {
    let next = {...this.state,...patch};
    if (patch.lastSeq !== undefined || patch.outbox || patch.daemonId !== undefined || patch.attention) next = saveSnapshot(this.storage,next,this.state,this.deviceId,lease);
    this.state = next; for (const listener of this.listeners) listener();
  }
  /** @param {string} id @param {'delivered'|'discarded'} [decision] */
  async dismissReceipt(id,decision) {
    string(id,'receipt ID'); const signal = this.submissionAbort.signal; const role = this.state.role;
    await this.withReceiptLock(id,()=>{
      signal.throwIfAborted();
      if (role !== this.state.role) throw new Error('Session changed before delivery decision');
      const row = mergeLedger(this.storage,this.state.daemonId,this.deviceId,this.state.outbox).find(item=>item.id===id);
      if (!row) throw new Error('Delivery receipt no longer exists');
      dismissReceipt(this.storage,this.state.daemonId,this.deviceId,row,decision);
      this.state = {...this.state,outbox:this.state.outbox.filter(item=>item.id!==id)};
      for (const listener of this.listeners) listener();
    },{signal,waitTimeout:SUBMISSION_LOCK_WAIT});
  }
  /** @template T @param {string} id @param {(lease:import('./submission-locks.js').SubmissionLock)=>T} action @param {import('./submission-locks.js').LockOptions} [options] */
  withReceiptLock(id,action,options = {}) {
    const scope = {daemonId:this.state.daemonId,deviceId:this.deviceId};
    return withSubmissionLock(scope,id,held=>{
      if (this.disposed || scope.daemonId !== this.state.daemonId || scope.deviceId !== this.deviceId) throw new Error('Delivery scope changed before lock acquisition');
      assertSubmissionLock(held,scope,id); return action(held);
    },options);
  }
  invalidateSubmissions() {
    ++this.epoch; this.submissionAbort.abort(new Error('Session changed before submission lock was granted'));
    this.submissionAbort = new AbortController(); this.recoveryQueue.clear();
    this.deferredRecovery.clear(); clearTimeout(this.recoveryTimer); this.recoveryTimer = undefined;
  }
  /** @param {string} id */
  deferSubmission(id) {
    string(id,'deferred receipt');
    if (this.disposed || this.state.connection !== 'online') return;
    this.deferredRecovery.add(id);
    if (this.recoveryTimer !== undefined) return;
    const epoch = this.epoch;
    this.recoveryTimer = setTimeout(()=>{
      this.recoveryTimer = undefined;
      if (epoch !== this.epoch || this.disposed) return;
      const ids = [...this.deferredRecovery]; this.deferredRecovery.clear();
      for (const deferred of ids) void this.resolvePendingSubmissions(deferred).catch(error=>this.patch({error:errorText(error)}));
    },1000);
  }
  /** @param {string} id */
  cancelDeferredSubmission(id) {
    this.deferredRecovery.delete(id);
    if (!this.deferredRecovery.size) { clearTimeout(this.recoveryTimer); this.recoveryTimer = undefined; }
  }
  /** @param {State['connection']} connection @param {string} connectionDetail */
  connection(connection, connectionDetail) {
    this.patch({connection,connectionDetail,...(connection !== 'online' ? {streams:{}} : {})});
    if (connection !== 'online') { this.invalidateSubmissions(); this.buffer = null; return; }
    void this.resolvePendingSubmissions().catch(error => this.patch({error:errorText(error)}));
    void this.startSubscription().catch(error => this.patch({error:errorText(error)}));
  }
  /** @param {string} [id] */
  resolvePendingSubmissions(id) {
    if (id) this.recoveryQueue.add(id);
    if (this.recovering && this.recoveryEpoch === this.epoch) return this.recovering;
    const epoch = this.epoch; this.recoveryEpoch = epoch;
    this.recovering = this.recoverQueued(epoch,id ? new Set([id]) : null).finally(() => { if (this.recoveryEpoch === epoch) this.recovering = null; });
    return this.recovering;
  }
  /** @param {number} epoch @param {Set<string>|null} ids */
  async recoverQueued(epoch, ids) {
    do {
      this.recoveryQueue.clear();
      await recoverSubmissions(this,epoch,ids);
      if (epoch !== this.epoch || this.state.connection !== 'online') return;
      ids = new Set(this.recoveryQueue);
    } while (ids.size);
  }
  async startSubscription() {
    const epoch = this.epoch;
    if (!this.state.lastSeq || this.needsResync) { await this.fullResync(epoch); return; }
    try {
      await this.transport.request({cmd:'subscribe',after:this.state.lastSeq,query:{},deltas:true});
    } catch (error) {
      if (!(error instanceof Error) || !('code' in error) || error.code !== 'not_found') throw error;
      await this.fullResync(epoch); return;
    }
    if (epoch !== this.epoch) return;
    await this.refresh();
    await this.refreshTranscripts();
  }
  /** @param {number} epoch */
  async fullResync(epoch) {
    this.buffer = [];
    try {
      const reply = await this.transport.request({cmd:'subscribe',after:null,query:{},deltas:true});
      if (reply.reply !== 'subscribed') throw new Error('Expected live subscription for resync');
      const baseline = integer(object(reply.data).last_seq,'subscription cursor');
      await this.refresh();
      await this.refreshTranscripts();
      if (epoch !== this.epoch || !this.buffer) return;
      const frames = this.buffer; this.buffer = null;
      this.patch({lastSeq:baseline,streams:{}}); this.needsResync = false;
      for (const frame of frames) this.receive(frame);
    } catch (error) { this.buffer = null; this.needsResync = true; throw error; }
  }
  /** @param {Record<string,unknown>} frame */
  receive(frame) {
    if (frame.frame === 'forget') {
      this.invalidateSubmissions();
      clearLocalState(this.storage); this.state = initialState(); this.deviceId = ''; this.completeTranscripts.clear(); this.drafts = createDrafts(this.storage);
      for (const listener of this.listeners) listener(); return;
    }
    if (frame.frame === 'resync') { this.needsResync = true; return; }
    if (this.buffer && (frame.frame === 'event' || frame.frame === 'delta')) {
      if (this.buffer.length >= 10_000) throw new Error('Full resync event buffer overflow');
      this.buffer.push(frame); return;
    }
    if (frame.frame === 'mutation') return this.withReceiptLock(string(frame.id),lease=>this.receiveReceipt(frame,lease),{lease:/** @type {import('./submission-locks.js').SubmissionLock|undefined} */ (frame.submissionLock)});
    if (frame.frame === 'session') {
      this.invalidateSubmissions();
      const daemonId = string(frame.daemonId); const deviceId = string(frame.deviceId);
      if (this.state.daemonId && (this.state.daemonId !== daemonId || this.deviceId !== deviceId)) { this.state = initialState(); this.completeTranscripts.clear(); this.attentionSeq = 0; }
      if (frame.role !== 'operator' && frame.role !== 'observer') throw new Error('Invalid session role');
      this.deviceId = deviceId;
      this.submissionLookup = frame.submissionHorizon != null;
      if (this.submissionLookup) string(frame.submissionHorizon,'submission horizon');
      const outbox = interruptedReceipts(mergeLedger(this.storage,daemonId,deviceId,this.state.outbox)).map(row=>['pending','unknown'].includes(row.status) ? {...row,recovery:/** @type {'resolving'|'unsupported'|'unknown'} */ (this.submissionLookup ? row.unconfirmed ? 'unknown' : 'resolving' : 'unsupported')} : row);
      this.patch({outbox,role:frame.role,daemonId,deviceName:String(frame.deviceName ?? ''),version:String(frame.version ?? '')}); return;
    }
    if (frame.frame === 'delta') { this.patch(applyDelta(this.state,frame)); return; }
    if (frame.frame !== 'event') return;
    const patch = applyEvent(this.state,frame.event);
    if (!patch) return;
    if (Number(object(frame.event).seq) <= this.attentionSeq) delete patch.attention;
    const receipts = patch.outbox; delete patch.outbox; this.patch(patch);
    if (receipts) for (const row of receipts) void Promise.resolve(this.withReceiptLock(row.id,lease=>{
      const current = mergeLedger(this.storage,this.state.daemonId,this.deviceId,this.state.outbox).find(item=>item.id===row.id);
      if (current && ['pending','unknown'].includes(current.status)) this.patch({outbox:this.state.outbox.map(item=>item.id===row.id ? {...current,retentionRisk:row.retentionRisk} : item)},lease);
    })).catch(error=>this.patch({error:errorText(error)}));
    if (!this.refreshTimer) this.refreshTimer = setTimeout(() => {
      this.refreshTimer = undefined;
      if (this.state.connection === 'online') void this.refresh().catch(error => this.patch({error:errorText(error)}));
    }, 250);
  }
  /** @param {Record<string,unknown>} frame @param {import('./submission-locks.js').SubmissionLock} lease */
  receiveReceipt(frame,lease) {
    assertSubmissionLock(lease,{daemonId:this.state.daemonId,deviceId:this.deviceId},string(frame.id));
    if (frame.late === true && !this.state.outbox.some(row=>row.id===frame.id)) return;
    const merged = mergeLedger(this.storage,this.state.daemonId,this.deviceId,this.state.outbox);
    const current = merged.find(row=>row.id===frame.id);
    if (frame.unsent === true && (!current || current.sendId !== frame.sendId)) return;
    if (frame.automaticRecovery === true && !lease.exclusive) throw new Error('Automatic resend requires Web Locks');
    if (frame.automaticRecovery === true && current?.unconfirmed) throw new Error(current.unconfirmed.message);
    if (this.storage && current && frame.phase === 'failed' && frame.sendId && frame.sendId !== current.sendId) {
      retire(this.storage,this.state.daemonId,this.deviceId,{...current,sendId:string(frame.sendId)},'failed');
    }
    const outbox = applyReceipt(merged,frame);
    try { this.patch({outbox},lease); }
    catch (error) {
      if (frame.phase === 'pending') throw error;
      this.state = {...this.state,outbox,error:`Receipt received but could not be saved: ${errorText(error)}`};
      for (const listener of this.listeners) listener();
    }
    if (frame.automaticRecovery === true && readUnconfirmed(this.storage,this.state.daemonId,this.deviceId,string(frame.id))) throw new Error('Delivery is unconfirmed. Choose whether to send again.');
    if (frame.phase === 'unknown' && this.state.outbox.some(row=>row.id===frame.id && row.status==='unknown') && this.state.connection === 'online') void this.resolvePendingSubmissions(string(frame.id)).catch(error=>this.patch({error:errorText(error)}));
    return;
  }
  refresh() {
    if (this.refreshing) { this.refreshAgain = true; return this.refreshing; }
    this.refreshing = this.refreshLoop().finally(() => { this.refreshing = null; });
    return this.refreshing;
  }
  async refreshLoop() {
    for (let attempt = 0; attempt < 2; attempt++) {
      this.refreshAgain = false; await this.fetchState();
      if (!this.refreshAgain) return;
    }
    if (!this.refreshTimer && this.state.connection === 'online') this.refreshTimer = setTimeout(() => {
      this.refreshTimer = undefined;
      if (this.state.connection === 'online') void this.refresh().catch(error => this.patch({error:errorText(error)}));
    },250);
  }
  async fetchState() {
    const epoch = this.epoch;
    const [agents,attention] = await Promise.all([this.transport.request({cmd:'list_agents',query:{}}),this.transport.request({cmd:'get_attention'})]);
    if (agents.reply !== 'agents' || attention.reply !== 'attention') throw new Error('Unexpected state reply');
    const normalizedAgents = unique(array(agents.data).map(agent), value => value.id);
    const data = object(attention.data); const seq = integer(data.last_seq);
    const normalizedAttention = unique([...array(data.approvals).map(approval),...array(data.questions).map(question),...array(data.agents).map(agentAttention)], value => value.id);
    if (epoch !== this.epoch) return;
    if (this.buffer || seq >= this.state.lastSeq) {
      this.attentionSeq = seq; this.patch({ready:true,agents:normalizedAgents,attention:normalizedAttention,error:null});
    } else { this.refreshAgain = true; this.patch({agents:normalizedAgents}); }
  }
  /** @param {string} id */
  loadTranscript(id) {
    string(id, 'agent ID');
    const existing = this.loading.get(id); if (existing) return existing;
    const transcripts = {...this.state.transcripts}; delete transcripts[id];
    while (Object.keys(transcripts).length >= 20) delete transcripts[Object.keys(transcripts)[0]];
    transcripts[id] = this.state.transcripts[id] ?? []; this.patch({transcripts});
    for (const cached of this.completeTranscripts) if (!Object.hasOwn(transcripts,cached)) this.completeTranscripts.delete(cached);
    const pending = this.fetchTranscript(id).finally(() => { this.loading.delete(id); });
    this.loading.set(id,pending); return pending;
  }
  /** @param {string} id */
  async fetchTranscript(id) {
    const epoch = this.epoch;
    const after = this.completeTranscripts.has(id) ? (this.state.transcripts[id] ?? []).reduce((seq,entry)=>Math.max(seq,entry.seq),0) : 0;
    const [reply,notes] = await Promise.all([this.transport.request({cmd:'get_transcript',agent:id,after}),this.fetchNotes(id,after)]);
    if (reply.reply !== 'transcript') throw new Error('Unexpected transcript reply');
    const records = unique([...array(reply.data).map(transcriptEntry),...notes], entry => entry.seq);
    if (epoch !== this.epoch || !Object.hasOwn(this.state.transcripts,id)) return;
    const merged = new Map(records.map(entry => [entry.seq,entry]));
    for (const entry of this.state.transcripts[id] ?? []) merged.set(entry.seq,entry);
    const transcripts = {...this.state.transcripts}; delete transcripts[id];
    while (Object.keys(transcripts).length >= 20) delete transcripts[Object.keys(transcripts)[0]];
    transcripts[id] = [...merged.values()].sort((a,b)=>a.seq-b.seq);
    this.completeTranscripts.add(id); this.patch({transcripts});
  }
  /** Every note of the agent after `after`, page by page: `get_transcript` holds its messages only. */
  /** @param {string} id @param {number} after */
  async fetchNotes(id, after) {
    /** @type {import('../types.js').TranscriptEntry[]} */ const notes = [];
    let cursor = after;
    for (let page = 0; page < NOTE_PAGES; page += 1) {
      // No event's type is `note.appended`, so `child_types` keeps children's events off the page.
      const reply = await this.transport.request({cmd:'get_events',after:cursor,limit:NOTE_PAGE,
        query:{agent:id,types:['custom'],child_types:['note.appended']}});
      if (reply.reply !== 'events') throw new Error('Unexpected note history reply');
      const records = array(reply.data, 'Note history');
      if (records.length > NOTE_PAGE) throw new Error('Note history exceeded the requested limit');
      for (const record of records) {
        const event = object(record, 'event'); const seq = integer(event.seq, 'event.seq');
        if (seq <= cursor) throw new Error('Note history is out of order');
        cursor = seq;
        const entry = event.agent_id === id ? noteEntry(event) : null;
        if (entry) notes.push(entry);
      }
      if (records.length < NOTE_PAGE) return notes;
    }
    throw new Error(`Note history is longer than the ${NOTE_PAGE * NOTE_PAGES} events a phone reads`);
  }
  async refreshTranscripts() {
    await Promise.all(Object.keys(this.state.transcripts).map(id => this.loadTranscript(id).catch(error => {
      this.patch({error:`Could not refresh transcript ${id}: ${errorText(error)}. Retained messages remain available.`});
    })));
  }
  /** @param {string} id @param {string} text */
  setDraft(id, text) { this.drafts.set(id,text); }
  /** @param {string} id */
  getDraft(id) { return this.drafts.get(id); }
  dispose() { if ('document' in globalThis) globalThis.removeEventListener('storage',this.storageListener); this.disposed = true; this.invalidateSubmissions(); clearTimeout(this.refreshTimer); for (const cleanup of this.cleanup) cleanup(); this.listeners.clear(); }
}
/** @param {Transport} transport @param {{storage?:Storage|null}} [options] */
export function createStore(transport, options) { return new RemoteStore(transport,options); }
