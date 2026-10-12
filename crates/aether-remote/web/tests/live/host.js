import net from 'node:net';
import {createInterface} from 'node:readline';

/** @param {string} socketPath @param {Record<string,unknown>} command */
export async function hostRequest(socketPath,command) {
  if (!socketPath.startsWith('/var/tmp/')) throw new Error('Live fixture requires an isolated /var/tmp socket');
  if (typeof command.cmd !== 'string') throw new Error('A host command is required');
  const socket = net.createConnection(socketPath);
  const lines = createInterface({input:socket});
  const timer = setTimeout(()=>socket.destroy(new Error(`Host request timed out: ${command.cmd}`)),15000);
  try {
    await new Promise((resolve,reject)=>{socket.once('connect',resolve);socket.once('error',reject);});
    socket.write(JSON.stringify({req:1,cmd:'hello',client_name:'aether-web-live-test',client_version:'1.0.1',protocol_version:18,features:[]})+'\n');
    for await (const line of lines) {
      const frame = JSON.parse(line);
      if (frame.frame !== 'reply') continue;
      if (frame.result.Err) throw new Error(`${command.cmd}: ${JSON.stringify(frame.result.Err)}`);
      if (frame.id === 1) socket.write(JSON.stringify({req:2,...command})+'\n');
      if (frame.id === 2) return frame.result.Ok;
    }
    throw new Error(`Host closed before replying to ${command.cmd}`);
  } finally { clearTimeout(timer); lines.close(); socket.destroy(); }
}

/** @template T @param {()=>Promise<T>} read @param {(value:T)=>boolean} accepts @param {string} description @param {number} [timeout] */
export async function until(read,accepts,description,timeout=15000) {
  if (!description || timeout <= 0) throw new Error('Polling requires a description and positive timeout');
  const end = Date.now()+timeout;
  let value;
  while (Date.now()<end) {
    value = await read();
    if (accepts(value)) return value;
    await new Promise(resolve=>setTimeout(resolve,100));
  }
  throw new Error(`Timed out waiting for ${description}: ${JSON.stringify(value)}`);
}
