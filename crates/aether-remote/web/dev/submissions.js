import { isDeepStrictEqual } from 'node:util';
import { KEYED_COMMANDS, assertAllowed } from '../core/contract.js';
import { object, string } from '../core/validate.js';
/** @typedef {import('../types.js').Command} Command */
/** @typedef {import('../types.js').Reply} Reply */
export class MockSubmissions {
  /** @param {Map<string,{command:Command,reply:Reply}>} receipts */
  constructor(receipts) {
    this.receipts = receipts; this.sequence = 1; this.retainedAfter = 0; this.enabled = true;
    this.retainedSince = new Date().toISOString();
    this.dropNextLookup = false; this.dropNextMutation = false;
    /** @type {string|null} */ this.rejectLookup = null;
  }
  get horizon() { return `mock-submissions:${this.sequence}`; }
  /** @param {Command} outer @param {{id:string,role:'operator'|'observer'}} device @returns {Reply} */
  resolve(outer, device) {
    if (!this.enabled) throw new Error('Submission lookup is disabled');
    const command = /** @type {Command} */ (object(outer.command));
    if (!KEYED_COMMANDS.has(command.cmd)) throw new Error('Submission lookup requires a keyed command');
    assertAllowed(command,device.role); const key = string(command.idempotency_key);
    if (this.rejectLookup) throw Object.assign(new Error('Mock submission lookup rejected'),{code:this.rejectLookup});
    const reason = this.uncertainty(outer.horizon);
    const saved = this.receipts.get(`${device.id}:${key}`);
    if (saved) {
      if (!isDeepStrictEqual(saved.command,command)) throw new Error('Idempotency key reused with different command');
      return {reply:'submission_resolved',data:{outcome:'executed',reply:structuredClone(saved.reply),horizon:this.horizon}};
    }
    return {reply:'submission_resolved',data:{outcome:reason ? 'unknown' : 'absent_safe',...(reason ? {reason} : {}),horizon:this.horizon,retained_since:this.retainedSince}};
  }
  /** @param {unknown} horizon */
  uncertainty(horizon) {
    if (horizon == null) return 'no_horizon';
    const value = string(horizon); const parts = /^([a-z-]+):(\d+)$/.exec(value);
    if (!parts || !Number.isSafeInteger(Number(parts[2]))) throw new Error('Malformed submission horizon');
    if (parts[1] !== 'mock-submissions') return 'foreign_horizon';
    const sequence = Number(parts[2]);
    if (sequence > this.sequence) return 'horizon_ahead';
    return sequence < this.retainedAfter ? 'forgotten' : null;
  }
  /** @param {Command} command */
  configure(command) {
    if (command.enabled !== undefined && typeof command.enabled !== 'boolean') throw new Error('Invalid mock capability');
    if (command.reject_lookup != null && !['invalid_argument','forbidden','not_found'].includes(String(command.reject_lookup))) throw new Error('Invalid mock lookup rejection');
    if (typeof command.enabled === 'boolean') this.enabled = command.enabled;
    this.dropNextLookup = command.drop_lookup === true; this.dropNextMutation = command.drop_mutation === true;
    this.rejectLookup = command.reject_lookup == null ? null : String(command.reject_lookup);
    if (command.forget === true) { this.receipts.clear(); this.retainedAfter = ++this.sequence; }
    return {reply:'ok'};
  }
}
