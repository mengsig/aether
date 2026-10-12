import { SUBMISSION_LOCK_WAIT, withSubmissionLock } from './submission-locks.js';
import { submissionResolution, welcomeHorizon } from './submissions.js';
import { checkAbort } from './abort.js';
import * as contract from './contract.js';
import * as device from './identity.js';
import { frame, object, string, errorText } from './validate.js';
/** @typedef {import('../types.js').Command} Command */
/** @typedef {import('../types.js').Reply} Reply */
/** @typedef {import('../types.js').State['connection']} Connection */
/** @typedef {{resolve:(reply:Reply)=>void,reject:(error:Error)=>void,mutation:boolean,timer:ReturnType<typeof setTimeout>}} Pending */
/** @typedef {{socket?:(url:string)=>WebSocket,identity?:typeof device,adapter?:typeof contract,random?:()=>number,timeout?:number,clientName?:string}} Options */
export class RemoteTransport {
  /** @param {Options} [options] */
  constructor(options = {}) {
    this.options = options;
    this.adapter = options.adapter ?? contract;
    this.device = options.identity ?? device;
    this.makeSocket = options.socket ?? (url => new WebSocket(url,'aether'));
    /** @type {Map<number,Pending>} */ this.pending = new Map();
    /** @type {Map<number,{command:Command,sendId?:string}>} */ this.lateMutations = new Map();
    /** @type {Set<(frame:Record<string,unknown>)=>void|Promise<void>>} */ this.frames = new Set();
    /** @type {Set<(status:Connection,detail:string)=>void>} */ this.connections = new Set();
    /** @type {WebSocket|null} */ this.socket = null;
    /** @type {Connection} */ this.status = 'offline';
    /** @type {'operator'|'observer'} */ this.role = 'observer';
    /** @type {ReturnType<typeof setTimeout>|undefined} */ this.retry = undefined;
    /** @type {Promise<void>|null} */ this.connecting = null;
    this.pairingAbort = new AbortController(); this.pairing = false;
    /** @type {ReturnType<typeof setInterval>|undefined} */ this.heartbeat = undefined;
    this.submissionScope = {daemonId:'',deviceId:''}; this.submissionAbort = new AbortController();
    this.stopped = true; this.generation = 0; this.attempt = 0; this.nextId = 0;
    this.reconnectDetail = '';
    /** @type {string|null} */ this.submissionHorizon = null;
  }
  /** @param {(frame:Record<string,unknown>)=>void|Promise<void>} listener */
  onFrame(listener) { this.frames.add(listener); return () => { this.frames.delete(listener); }; }
  /** @param {(status:Connection,detail:string)=>void} listener */
  onConnection(listener) { this.connections.add(listener); return () => { this.connections.delete(listener); }; }
  /** @param {Connection} status @param {string} detail */
  connection(status, detail) {
    if (status !== 'online') { this.submissionAbort.abort(new Error('Connection changed before submission lock was granted')); this.submissionAbort = new AbortController(); }
    this.status = status; for (const listener of this.connections) listener(status, detail);
  }
  connect() {
    if (this.connecting) return this.connecting;
    if (this.status === 'online' || this.pairing) return Promise.resolve();
    this.stopped = false; clearTimeout(this.retry); this.retry = undefined;
    const generation = ++this.generation;
    this.connecting = this.open(generation).finally(() => { this.connecting = null; });
    return this.connecting;
  }
  /** @param {number} generation */
  async open(generation) {
    this.connection(this.attempt ? 'reconnecting' : 'connecting', 'Authenticating this device');
    try {
      const identity = await this.device.loadIdentity();
      if (generation !== this.generation) return;
      if (!identity?.deviceId) { this.stopped = true; this.connection('unpaired', 'Pair this browser with your daemon'); return; }
      const challenge = await this.openSocket(this.adapter.socketURL(), generation);
      if (generation !== this.generation) return;
      const hello = await this.adapter.authenticate(identity,challenge,undefined,this.options.clientName ?? (!('document' in globalThis) ? 'aether-web-sw' : 'aether-web'));
      if (generation !== this.generation) return;
      const welcome = await this.send(hello);
      if (welcome.reply !== 'welcome') throw new Error('Daemon did not return a welcome');
      const data = object(welcome.data); const authenticated = object(data.device);
      if (authenticated.id !== identity.deviceId || data.instance_id !== challenge.instance_id || data.protocol_version !== contract.PROTOCOL_VERSION) throw new Error('Invalid authenticated welcome');
      if (authenticated.role !== 'operator' && authenticated.role !== 'observer') throw new Error('Invalid authenticated device role');
      this.submissionScope = {daemonId:string(data.instance_id),deviceId:identity.deviceId};
      this.role = authenticated.role; this.submissionHorizon = welcomeHorizon(data);
      const daemonId = string(data.instance_id); const deviceName = string(authenticated.name);
      await this.device.saveIdentity({...identity,daemonId,role:this.role,name:deviceName});
      if (generation !== this.generation) return;
      this.startHeartbeat(data.idle_timeout_secs);
      for (const listener of this.frames) listener({frame:'session',deviceId:identity.deviceId,role:this.role,deviceName,daemonId,version:data.daemon_version,submissionHorizon:this.submissionHorizon});
      this.attempt = 0; this.reconnectDetail = ''; this.connection('online', 'Connected');
    } catch (error) {
      if (generation !== this.generation) return;
      if (this.stopped && ['revoked','identity-changed'].includes(this.status)) throw error;
      const code = error instanceof Error && 'code' in error ? error.code : '';
      if (code === 'identity_changed' || code === 'revoked' || code === 'unauthorized' || code === 'forbidden' || code === 'incompatible') {
        this.stopped = true; this.connection(code === 'identity_changed' ? 'identity-changed' : 'revoked', errorText(error));
      } else {
        this.reconnectDetail = code === 'unavailable' ? 'All device connections are in use. Retrying shortly.' : errorText(error);
        if (code === 'unavailable') { clearTimeout(this.retry); this.retry = undefined; this.attempt = Math.max(this.attempt,3); }
        this.connection('reconnecting', this.reconnectDetail); this.scheduleReconnect();
      }
      this.socket?.close();
      throw error;
    }
  }
  /** @param {string} url @param {number} generation @returns {Promise<Record<string,unknown>>} */
  openSocket(url, generation) {
    return new Promise((resolve, reject) => {
      const socket = this.makeSocket(url); this.socket = socket; let challenged = false;
      const timer = setTimeout(() => { socket.close(); reject(new Error('Challenge timed out')); }, this.options.timeout ?? 15_000);
      socket.onerror = () => { clearTimeout(timer); reject(new Error('WebSocket connection failed')); };
      socket.onmessage = message => {
        if (generation !== this.generation) return;
        if (challenged) { this.receive(message.data); return; }
        try {
          const challenge = frame(message.data);
          if (challenge.frame !== 'challenge') throw new Error('Expected authentication challenge before hello');
          challenged = true; clearTimeout(timer); resolve(challenge);
        } catch (error) { clearTimeout(timer); reject(error); socket.close(1002,'Invalid challenge'); }
      };
      socket.onclose = close => {
        const detail = close.code === 4408 ? 'Connection timed out. Reconnecting…' : 'Connection interrupted. Reconnecting…';
        clearTimeout(timer); clearInterval(this.heartbeat); reject(new Error(this.reconnectDetail || detail));
        if (generation !== this.generation) return;
        this.socket = null; this.rejectPending('Connection closed before a receipt arrived');
        if (this.stopped) return;
        if (close.code === 1003 || close.code === 1009) {
          this.stopped = true; this.connection('offline','WebSocket framing was rejected'); return;
        }
        if (close.code === 4401) {
          this.stopped = true; this.connection('revoked', 'Device access was refused or revoked'); return;
        }
        if (close.code === 4503) this.attempt = 6;
        if (close.code === 4408) this.attempt = Math.max(this.attempt,2);
        this.connection('reconnecting', this.reconnectDetail || detail); this.scheduleReconnect();
      };
    });
  }
  /** @param {unknown} idle */
  startHeartbeat(idle) {
    clearInterval(this.heartbeat);
    if (idle == null) return;
    if (typeof idle !== 'number' || !Number.isSafeInteger(idle) || idle < 1) throw new Error('Invalid server idle timeout');
    this.heartbeat = setInterval(() => {
      if (this.status !== 'online') return;
      void this.request({cmd:'ping'}).catch(error => { this.connection('reconnecting',errorText(error)); this.socket?.close(); });
    },Math.max(250,Math.min(20_000,idle*500)));
  }
  scheduleReconnect() {
    if (this.stopped || this.retry) return;
    const random = this.options.random ?? Math.random;
    const delay = Math.min(30_000,Math.floor(Math.min(30_000, 1000 * 2 ** Math.min(this.attempt++, 6)) * (0.5 + random())));
    this.retry = setTimeout(() => {
      this.retry = undefined;
      void this.connect().catch(error => { if (!this.stopped) this.connection('reconnecting', errorText(error)); });
    }, delay);
  }
  /** @param {Command} command @param {import('../types.js').RequestOptions} [options] */
  async request(command,options = {}) {
    string(command.cmd, 'command name');
    if (this.status !== 'online') throw new Error('Daemon is not connected');
    contract.assertAllowed(command,this.role);
    if (options.recovery && !options.submissionLock) throw new Error('Automatic recovery requires its active submission lock');
    if (options.recovery && !contract.KEYED_COMMANDS.has(command.cmd)) throw new Error('Automatic recovery requires a keyed mutation');
    if (command.cmd === 'resolve_submission') return this.resolveSubmission(command);
    if (!contract.isMutation(command)) return this.send(command);
    return this.mutate(command,options);
  }
  /** @param {Command} command */
  async resolveSubmission(command) {
    if (this.submissionHorizon === null) throw new Error('This daemon does not support submission lookup');
    if (command.idempotency_key !== undefined) throw new Error('Submission lookup cannot carry an outer idempotency key');
    const generation = this.generation;
    try {
      const reply = await this.send(command); const resolution = submissionResolution(reply);
      if (generation === this.generation) this.submissionHorizon = resolution.horizon;
      return reply;
    } catch (error) {
      if (generation === this.generation && error instanceof Error && 'outcome' in error && !('code' in error)) this.socket?.close();
      throw error;
    }
  }
  /** @param {Command} input @param {import('../types.js').RequestOptions} [options] */
  mutate(input,options = {}) {
    const keyed = contract.KEYED_COMMANDS.has(input.cmd);
    const command = structuredClone(keyed ? {...input,idempotency_key:input.idempotency_key ?? crypto.randomUUID()} : input);
    const id = command.idempotency_key ?? crypto.randomUUID(); const generation = this.generation;
    const scope = this.submissionScope; const role = this.role; const signal = this.submissionAbort.signal;
    return withSubmissionLock(scope,id,lease=>{
      signal.throwIfAborted();
      if (generation !== this.generation || scope !== this.submissionScope || this.status !== 'online' || role !== this.role) throw new Error('Connection changed before submission lock was granted');
      contract.assertAllowed(command,this.role);
      if (options.recovery && !lease.exclusive) throw new Error('Automatic resend requires Web Locks');
      return this.mutateLocked(command,id,lease,options.recovery === true);
    },{lease:options.submissionLock,signal,waitTimeout:SUBMISSION_LOCK_WAIT});
  }
  /** @param {Record<string,unknown>} frame */
  emitMutation(frame) {
    const waiting = [];
    for (const listener of this.frames) { const result = listener(frame); if (result instanceof Promise) waiting.push(result); }
    if (waiting.length) return Promise.all(waiting).then(()=>{});
  }
  /** @param {Command} command @param {string} id @param {import('./submission-locks.js').SubmissionLock} submissionLock @param {boolean} automaticRecovery */
  async mutateLocked(command,id,submissionLock,automaticRecovery) {
    const sendId = crypto.randomUUID(); const generation = this.generation; const role = this.role;
    const receipt = {frame:'mutation',id,command,sendId,submissionLock};
    try {
      const waiting = this.emitMutation({...receipt,phase:'pending',automaticRecovery,horizon:this.submissionHorizon});
      if (waiting) await waiting;
      if (generation !== this.generation || role !== this.role || this.status !== 'online') throw new Error('Connection changed before transmission');
      contract.assertAllowed(command,this.role);
    } catch (error) {
      await this.emitMutation({...receipt,phase:'failed',unsent:true,error:`Not sent: ${errorText(error)}`});
      throw error;
    }
    try {
      const reply = await this.send(command,sendId);
      await this.emitMutation({...receipt,phase:'sent',reply});
      return reply;
    } catch (error) {
      const unknown = error instanceof Error && 'outcome' in error && error.outcome === 'unknown';
      await this.emitMutation({...receipt,phase:unknown ? 'unknown' : 'failed',error:errorText(error)});
      throw error;
    }
  }
  /** @param {Command} command @param {string} [sendId] @returns {Promise<Reply>} */
  send(command,sendId) {
    const socket = this.socket;
    if (!socket || socket.readyState !== 1) return Promise.reject(new Error('WebSocket is not open'));
    if (this.pending.size + this.lateMutations.size >= 128 || this.nextId >= Number.MAX_SAFE_INTEGER) return Promise.reject(new Error('Request capacity exceeded'));
    const req = ++this.nextId;
    const encoded = JSON.stringify({...command,req});
    if (new TextEncoder().encode(encoded).length > 67_108_864) return Promise.reject(new Error('Command exceeds frame limit'));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(req);
        if (contract.isMutation(command)) this.lateMutations.set(req,{command,sendId});
        reject(deliveryError('Request timed out before a receipt arrived', contract.isMutation(command)));
      }, this.options.timeout ?? 20_000);
      this.pending.set(req, {resolve,reject,timer,mutation:contract.isMutation(command)});
      try { socket.send(encoded); }
      catch (error) { clearTimeout(timer); this.pending.delete(req); reject(deliveryError(errorText(error), contract.isMutation(command))); }
    });
  }
  /** @param {unknown} raw */
  receive(raw) {
    try {
      const value = frame(raw);
      if (value.frame === 'challenge') throw new Error('Unexpected repeated authentication challenge');
      if (value.frame !== 'reply') { for (const listener of this.frames) listener(value); return; }
      const result = object(value.result);
      if (value.id === 0) { this.diagnostic(object(result.Err)); return; }
      const pending = this.pending.get(Number(value.id));
      if (!pending) {
        void this.receiveLate(Number(value.id),result).catch(error=>{ this.connection('offline',`Could not save late receipt: ${errorText(error)}`); this.socket?.close(); }); return;
      }
      clearTimeout(pending.timer); this.pending.delete(Number(value.id));
      if (result.Err) {
        const error = object(result.Err); pending.reject(Object.assign(new Error(String(error.message)), {code:error.code,outcome:'failed'}));
      } else pending.resolve(/** @type {Reply} */ (result.Ok));
    } catch (error) {
      this.stopped = true; this.rejectPending(errorText(error));
      this.connection('offline', `Protocol error: ${errorText(error)}`); this.socket?.close(1002, 'Invalid server frame');
    }
  }
  /** @param {number} req @param {Record<string,unknown>} result */
  async receiveLate(req, result) {
    const pending = this.lateMutations.get(req);
    if (!pending) return;
    const {command,sendId} = pending;
    this.lateMutations.delete(req);
    if (!command.idempotency_key) return;
    const error = result.Err ? object(result.Err) : null;
    const scope = this.submissionScope;
    await withSubmissionLock(scope,command.idempotency_key,submissionLock=>this.emitMutation({frame:'mutation',phase:error ? 'failed' : 'sent',late:true,id:command.idempotency_key,command,sendId,submissionLock,reply:result.Ok,error:error?.message}));
  }
  /** @param {Record<string,unknown>} error */
  diagnostic(error) {
    const message = string(error.message); const code = string(error.code);
    this.rejectPending(message);
    if (['unauthorized','incompatible'].includes(code)) {
      this.stopped = true; this.connection('revoked',message);
    } else {
      for (const listener of this.frames) listener({frame:'resync'});
      this.connection('reconnecting',`Resynchronizing after daemon error: ${message}`);
    }
    this.socket?.close();
  }
  /** @param {string} message */
  rejectPending(message) {
    for (const pending of this.pending.values()) { clearTimeout(pending.timer); pending.reject(deliveryError(message, pending.mutation)); }
    this.pending.clear(); this.lateMutations.clear();
  }
  disconnect() { this.halt(); this.connection('offline', 'Disconnected'); }
  halt() {
    this.pairingAbort.abort(new Error('Pairing cancelled')); clearInterval(this.heartbeat);
    this.stopped = true; ++this.generation; clearTimeout(this.retry); this.retry = undefined;
    this.rejectPending('Disconnected before a receipt arrived'); this.socket?.close(); this.socket = null;
  }
  /** @param {string} secret @param {string} name @param {(progress:contract.PairProgress)=>void} [progress] */
  async pair(secret, name, progress) {
    this.halt(); this.pairingAbort = new AbortController();
    const signal = this.pairingAbort.signal;
    // Pairing reads as unpaired, never offline, so the screen showing the approval words stays up.
    this.pairing = true; this.connection('unpaired', 'Waiting for approval on your host');
    try {
      const identity = await this.device.newIdentity();
      const paired = await this.adapter.pairDevice(identity, secret, name, progress, signal);
      checkAbort(signal);
      await this.device.saveIdentity(paired);
      // A forget that landed during the save must win: never leave its identity behind.
      if (signal.aborted) { await this.device.deleteIdentity(); checkAbort(signal); }
    } catch (error) {
      if (!signal.aborted) this.connection('unpaired', 'Pairing did not finish');
      throw error;
    } finally { if (this.pairingAbort.signal === signal) this.pairing = false; }
    await this.connect();
  }
  async forget() { this.disconnect(); await this.device.deleteIdentity(); for (const listener of this.frames) listener({frame:'forget'}); this.connection('unpaired', 'This browser has been unpaired'); }
}
/** @param {string} message @param {boolean} mutation */
function deliveryError(message, mutation) {
  return Object.assign(new Error(mutation ? `Outcome unknown: ${message}. Inspect daemon state before retrying.` : message), {outcome:mutation ? 'unknown' : 'failed'});
}
/** @param {Options} [options] */
export function createTransport(options) { return new RemoteTransport(options); }
