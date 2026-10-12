import { randomBytes, randomUUID, createPublicKey, createHash, verify } from 'node:crypto';
import { object, string } from '../core/validate.js';
/** @typedef {{id:string,name:string,role:'operator'|'observer',publicKey:import('node:crypto').KeyObject,fingerprint:string,revoked:boolean}} Device */
/** @typedef {{nonce:string,instance_id:string,origin:string}} Challenge */
/** @typedef {{device:Device,state:string,phrase:string,expires:number,confirmAfter:number}} Pairing */
export class MockAuth {
  constructor() {
    this.daemonId = 'mock-daemon-aether';
    /** @type {Map<string,Device>} */ this.devices = new Map();
    /** @type {Map<string,Pairing>} */ this.pairings = new Map();
    /** @type {Map<string,number>} */ this.challenges = new Map();
  }
  /** @param {Record<string,unknown>} data */
  claim(data) {
    if (data.code !== (process.env.PAIR_SECRET ?? 'aether-demo') && data.code !== 'aether-observer') throw new Error('Invalid pairing code');
    if (this.devices.size + this.pairings.size >= 100) throw new Error('Mock pairing capacity reached');
    const name = string(data.device_name);
    if (name.length > 64 || /[\x00-\x1f\x7f]/.test(name)) throw new Error('Invalid device name');
    const raw = Buffer.from(string(data.public_key),'base64url');
    if (raw.length !== 65 || raw[0] !== 4) throw new Error('Expected an uncompressed P-256 public key');
    const publicKey = createPublicKey({key:{kty:'EC',crv:'P-256',x:raw.subarray(1,33).toString('base64url'),y:raw.subarray(33).toString('base64url')},format:'jwk'});
    const fingerprint = createHash('sha256').update(raw).digest('hex'); const pairing = randomUUID();
    const role = data.code === 'aether-observer' ? 'observer' : 'operator';
    const device = {id:`dev_${randomBytes(13).toString('hex').toUpperCase()}`,name,role:/** @type {'operator'|'observer'} */ (role),publicKey,fingerprint,revoked:false};
    const phrase = 'amber-otter-canyon'; const expires = Date.now()+300_000;
    this.pairings.set(pairing,{device,state:'claimed',phrase,expires,confirmAfter:Date.now()+1500});
    return {pairing,fingerprint,phrase,expires_at:new Date(expires).toISOString()};
  }
  /** @param {Record<string,unknown>} data */
  status(data) {
    const pairing = this.pairings.get(string(data.pairing));
    if (!pairing) throw new Error('Pairing not found');
    if (pairing.expires < Date.now()) pairing.state = 'expired';
    if (pairing.state === 'claimed' && process.env.MOCK_AUTO_CONFIRM !== '0' && Date.now() >= pairing.confirmAfter) this.confirm(String(data.pairing),true);
    const {id,name,role} = pairing.device;
    return {state:pairing.state,...(pairing.state === 'enrolled' ? {device:{id,name,role}} : {})};
  }
  /** @param {string} id @param {boolean} accept */
  confirm(id, accept) {
    const pairing = this.pairings.get(id);
    if (!pairing || pairing.state !== 'claimed' || pairing.expires < Date.now()) throw new Error('Pairing is no longer claimable');
    pairing.state = accept ? 'enrolled' : 'rejected'; if (accept) this.devices.set(pairing.device.id,pairing.device);
  }
  /** @param {string} origin @returns {Challenge} */
  challenge(origin) {
    for (const [nonce,expires] of this.challenges) if (expires < Date.now()) this.challenges.delete(nonce);
    if (this.challenges.size >= 1000) throw new Error('Too many authentication requests');
    const nonce = randomBytes(32).toString('base64url'); this.challenges.set(nonce,Date.now()+30_000);
    return {nonce,instance_id:this.daemonId,origin};
  }
  /** @param {unknown} value @param {Challenge} challenge */
  authenticate(value, challenge) {
    const data = object(value); const device = this.device(data.id); const expires = this.challenges.get(challenge.nonce);
    this.challenges.delete(challenge.nonce);
    if (!expires || expires < Date.now()) throw new Error('Challenge expired or invalid');
    const signature = Buffer.from(string(data.signature),'base64url');
    const input = ['aether-remote-auth-v1',challenge.instance_id,challenge.origin,device.id,challenge.nonce].join('\n');
    if (signature.length !== 64 || !verify('sha256',Buffer.from(input),{key:device.publicKey,dsaEncoding:'ieee-p1363'},signature)) throw new Error('Invalid device signature');
    return device;
  }
  /** @param {unknown} id */
  device(id) {
    const device = this.devices.get(string(id));
    if (!device || device.revoked) throw new Error('Device revoked or unknown'); return device;
  }
}
