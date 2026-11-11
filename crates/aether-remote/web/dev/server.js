import { createServer } from 'node:http';
import { pathToFileURL } from 'node:url';
import { isDeepStrictEqual } from 'node:util';
import { WebSocketServer } from 'ws';
import { assertProtocol, validateCommand } from './validate.js';
import { MockSubmissions } from './submissions.js';
import { MockAuth } from './auth.js';
import { serve, sameOrigin } from './http.js';
import { createFixtures, mockId } from './fixtures.js';
import { readCommand, mutateCommand } from './commands.js';
import { assertAllowed } from '../core/contract.js';
import { object, integer, string, errorText } from '../core/validate.js';
/** @typedef {import('ws').WebSocket} Socket */
/** @typedef {{device:import('./auth.js').Device|null,challenge:import('./auth.js').Challenge,sub:number|null,hello:boolean}} Session */
export class MockServer {
  constructor() {
    this.auth = new MockAuth(); this.state = createFixtures();
    this.http = createServer((request,response) => { void serve(request,response,this.auth); });
    this.wss = new WebSocketServer({noServer:true,maxPayload:67_108_864,perMessageDeflate:false});
    /** @type {Map<Socket,Session>} */ this.clients = new Map();
    /** @type {Map<string,{command:import('../types.js').Command,reply:import('../types.js').Reply}>} */ this.receipts = new Map();
    this.submissions = new MockSubmissions(this.receipts);
    this.http.on('upgrade',(request,socket,head) => this.upgrade(request,socket,head));
  }
  /** @param {import('node:http').IncomingMessage} request @param {import('node:stream').Duplex} socket @param {Buffer} head */
  upgrade(request, socket, head) {
    try {
      if (request.headers['sec-websocket-protocol'] !== 'aether') throw new Error('The aether WebSocket subprotocol is required');
      if (!sameOrigin(request)) throw new Error('Unexpected WebSocket origin');
      const url = new URL(request.url ?? '/',`http://${request.headers.host}`);
      if (url.pathname !== '/ws' || url.search) throw new Error('Unknown WebSocket route');
      const challenge = this.auth.challenge(string(request.headers.origin));
      this.wss.handleUpgrade(request,socket,head,ws => {
        this.clients.set(ws,{device:null,challenge,sub:null,hello:false});
        ws.send(JSON.stringify({frame:'challenge',nonce:challenge.nonce,instance_id:challenge.instance_id}));
        ws.on('message',(data,binary) => this.handle(ws,binary ? null : data.toString()));
        ws.on('close',() => this.clients.delete(ws));
        ws.on('error',error => console.error('Mock socket error:',error.message));
      });
    } catch (error) { console.error('Mock upgrade rejected:',errorText(error)); socket.end('HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n'); }
  }
  /** @param {Socket} ws @param {unknown} raw */
  handle(ws, raw) {
    let req = 0;
    try {
      const session = this.clients.get(ws); if (!session) throw new Error('Session missing');
      if (session.device?.revoked) { ws.close(4403,'Device revoked'); return; }
      if (typeof raw !== 'string') throw new Error('Expected text WebSocket frame');
      const input = object(JSON.parse(raw)); req = integer(input.req); if (!req) throw new Error('Request ID must be positive');
      validateCommand(input);
      const command = /** @type {import('../types.js').Command} */ ({...input,cmd:string(input.cmd)}); delete command.req;
      if (!session.hello && command.cmd !== 'hello') throw new Error('Hello required');
      if (command.cmd !== 'hello') {
        if (!session.device) throw new Error('Device authentication required');
        try { assertAllowed(command,session.device.role); }
        catch (cause) { throw Object.assign(new Error(errorText(cause),{cause}),{code:'forbidden'}); }
      }
      if (command.cmd === 'mock_disconnect') { ws.close(1012,'Reconnect test'); return; }
      if (command.cmd === 'mock_malformed') { ws.send('{bad-json'); return; }
      if (this.dropSubmission(ws,command)) return;
      const reply = this.execute(command,session,req); assertProtocol('Reply',reply);
      if (command.mock_drop_reply === true) { ws.close(1012,'Receipt lost after commit'); return; }
      ws.send(JSON.stringify({frame:'reply',id:req,result:{Ok:reply}}));
      if (command.cmd === 'subscribe') this.replay(ws,command.after == null ? this.state.seq : Number(command.after),req);

    } catch (error) {
      ws.send(JSON.stringify({frame:'reply',id:req,result:{Err:{code:error instanceof Error && 'code' in error ? error.code : clientsHello(this.clients.get(ws)) ? 'invalid_argument' : 'unauthorized',message:errorText(error)}}}));
      if (!req || !clientsHello(this.clients.get(ws))) ws.close(clientsHello(this.clients.get(ws)) ? 1002 : 4401,'Invalid request');
    }
  }
  /** @param {import('../types.js').Command} command @param {Session} session @param {number} req @returns {import('../types.js').Reply} */
  execute(command, session, req) {
    if (command.cmd === 'hello') {
      if (command.protocol_version !== 18) throw Object.assign(new Error('Protocol version must be 18'),{code:'incompatible_protocol'});
      if (session.hello) throw new Error('Hello was already authenticated');
      session.device = this.auth.authenticate(command.device,session.challenge);
      const {id,name,role} = session.device;
      session.hello = true; return {reply:'welcome',data:{daemon_version:'mock-1.0.1',protocol_version:18,device:{id,name,role},instance_id:this.auth.daemonId,schema_version:1,pid:process.pid,features:[],...(this.submissions.enabled ? {submission_horizon:this.submissions.horizon} : {})}};
    }
    if (!session.device) throw new Error('Device authentication required');
    if (command.cmd === 'resolve_submission') return this.submissions.resolve(command,session.device);
    if (command.cmd === 'mock_submission_fault') return this.submissions.configure(command);
    if (command.cmd === 'subscribe') { session.sub = req; return {reply:'subscribed',data:{id:req,last_seq:this.state.seq}}; }
    if (command.cmd === 'unsubscribe') { session.sub = null; return {reply:'ok'}; }
    if (command.cmd === 'list_devices') return {reply:'devices',data:[...this.auth.devices.values()].filter(device=>!device.revoked).map(({id,name,role})=>({id,name,role,created_at:'2026-10-12T08:30:00Z',current:id===session.device?.id,connections:1,push:false}))};
    if (command.cmd === 'revoke_device') {
      this.auth.device(command.id).revoked = true;
      for (const [ws,client] of this.clients) if (client.device?.id === command.id) ws.close(4403,'Device revoked');
      return {reply:'ok'};
    }
    const read = readCommand(this.state,command); if (read) return read;
    const key = command.idempotency_key ? `${session.device.id}:${command.idempotency_key}` : '';
    const saved = key ? this.receipts.get(key) : null;
    if (saved) { if (!isDeepStrictEqual(saved.command,command)) throw new Error('Idempotency key reused with different command'); return structuredClone(saved.reply); }
    const result = mutateCommand(this.state,command);
    if (['send_message','reply_to_question'].includes(command.cmd)) this.broadcastMessage(command,string(object(result.data).message));
    if (key) {
      ++this.submissions.sequence;
      this.receipts.set(key,{command:structuredClone(command),reply:structuredClone(result)});
      if (this.receipts.size > 1000) {
        this.receipts.delete(/** @type {string} */ (this.receipts.keys().next().value)); this.submissions.retainedAfter = this.submissions.sequence;
      }
    }
    return result;
  }
  /** @param {Socket} ws @param {import('../types.js').Command} command */
  dropSubmission(ws, command) {
    const lookup = command.cmd === 'resolve_submission' && this.submissions.dropNextLookup;
    const mutation = Boolean(command.idempotency_key) && this.submissions.dropNextMutation;
    if (!lookup && !mutation) return false;
    if (lookup) this.submissions.dropNextLookup = false;
    if (mutation) this.submissions.dropNextMutation = false;
    ws.close(1012,lookup ? 'Submission lookup reply lost' : 'Mutation lost before commit'); return true;
  }
  /** @param {Socket} ws @param {number} after @param {number} sub */
  replay(ws, after, sub) {
    if (!Number.isSafeInteger(after) || after < 0) throw new Error('Invalid replay cursor');
    for (const event of this.state.events) if (event.seq > after) ws.send(JSON.stringify({frame:'event',sub,event}));
  }
  /** @param {import('../types.js').Command} command @param {string} message */
  broadcastMessage(command, message) {
    const at = new Date().toISOString(); const seq = ++this.state.seq;
    const event = {id:mockId('evt',seq),seq,at,agent_id:String(command.to),actor:{actor:'user'},kind:{type:'message.sent',message,from:{actor:'user'},to:command.to,content:[{type:'text',text:command.content}]}};
    assertProtocol('Event',event);
    this.state.events.push(/** @type {typeof this.state.events[number]} */ (/** @type {unknown} */ (event)));
    for (const [ws,session] of this.clients) {
      if (session.sub === null || ws.readyState !== 1) continue;
      ws.send(JSON.stringify({frame:'event',sub:session.sub,event}));
      ws.send(JSON.stringify({frame:'delta',sub:session.sub,agent:command.to,invocation:mockId('inv',seq),delta:{delta:'text',text:'Received. I’m reviewing the request and current state…'}}));
    }
  }
  async close() {
    for (const ws of this.clients.keys()) ws.terminate();
    await new Promise((resolve,reject) => this.wss.close(error => error ? reject(error) : resolve(undefined)));
    if (!this.http.listening) return;
    const closed = new Promise((resolve,reject) => this.http.close(error => error ? reject(error) : resolve(undefined)));
    // close() waits for ever on a connection that never sent a request, and a browser keeps a spare one.
    this.http.closeAllConnections();
    await closed;
  }
}
/** @param {Session|undefined} session */
function clientsHello(session) { return session?.hello === true; }
export function createMockServer() { return new MockServer(); }
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const port = Number(process.env.PORT ?? 4173);
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('PORT must be between 1 and 65535');
  const server = createMockServer();
  server.http.listen(port,'127.0.0.1',() => console.log(`Aether mock: http://127.0.0.1:${port} · pairing secret: ${process.env.PAIR_SECRET ?? 'aether-demo'} (observer: aether-observer)`));
}
