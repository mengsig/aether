import {spawn,execFileSync} from 'node:child_process';
import {mkdir,mkdtemp,writeFile,readFile,rm,copyFile} from 'node:fs/promises';
import {openSync,closeSync,appendFileSync} from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import {hostRequest,until} from './host.js';

const web = path.resolve(import.meta.dirname,'../..');
const repository = path.resolve(web,'../../..');

async function freePort() {
  const server = net.createServer();
  await new Promise(resolve=>server.listen(0,'127.0.0.1',()=>resolve(undefined)));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('Could not reserve loopback port');
  const port = address.port;
  await new Promise((resolve,reject)=>server.close(error=>error?reject(error):resolve(undefined)));
  return port;
}

/** @param {import('node:child_process').ChildProcess} child */
async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const ended = new Promise(resolve=>child.once('exit',resolve));
  child.kill('SIGTERM');
  const timer = setTimeout(()=>child.kill('SIGKILL'),5000);
  await ended; clearTimeout(timer);
}

/** @param {string} binary */
export async function liveFixture(binary) {
  if (!path.isAbsolute(binary)) throw new Error('AETHER_BINARY must be an absolute path');
  const root = await mkdtemp('/var/tmp/aw-');
  /** @type {import('node:child_process').ChildProcess[]} */ const children=[];
  try {
    const workspace = path.join(root,'workspace');
    const socket = path.join(root,'daemon.sock');
    const config = path.join(root,'config'); const data = path.join(root,'data');
    /** @type {NodeJS.ProcessEnv} */
    const environment = {...process.env,AETHER_CONFIG_DIR:config,AETHER_DATA_DIR:data,AETHER_SOCKET:socket,PYTHONDONTWRITEBYTECODE:'1'};
    delete environment.AETHER_REMOTE; delete environment.AETHER_ADDRESS; delete environment.AETHER_TOKEN;
    for (const directory of [workspace,config,data]) await mkdir(directory);
    const providerPort = await freePort(); const port = await freePort();
    await writeFile(path.join(workspace,'README.md'),'# Mobile acceptance\n\nA sample workspace for remote controls.\n');
    execFileSync('git',['init','-q',workspace]);
    execFileSync('git',['-C',workspace,'add','README.md']);
    execFileSync('git',['-C',workspace,'-c','user.name=Aether fixture','-c','user.email=fixture@localhost','commit','-qm','Initial fixture']);
    await writeFile(path.join(config,'providers.toml'),providerConfig(providerPort));
    await writeFile(path.join(config,'config.toml'),`[model]\ndefault = "mock/mock-1"\n[remote]\naddress = "127.0.0.1:${port}"\npush_hosts = ["push.test.invalid"]\n`);
    const mock = launch('python3',[path.join(web,'tests/live/provider.py'),path.join(repository,'scripts/mock_openai.py'),String(providerPort)],'mock',root,environment,workspace);
    children.push(mock);
    await until(()=>readFile(path.join(root,'mock.log'),'utf8'),text=>text.includes('mock openai server'),'mock provider');
    let daemon = launch(binary,['--local','daemon','--foreground'],'daemon',root,environment,workspace);
    children.push(daemon);
    const start = async () => {
      await until(()=>readFile(path.join(root,'daemon.log'),'utf8'),text=>text.includes('listening'),'daemon socket');
      execFileSync(binary,['--local','remote','enable'],{env:environment,cwd:workspace,stdio:'pipe'});
    };
    await start();
    return {root,workspace,socket,url:`http://127.0.0.1:${port}`,request:(/** @type {Record<string,unknown>} */ command)=>hostRequest(socket,command),
      restart:async()=>{await stop(daemon);daemon=launch(binary,['--local','daemon','--foreground'],'daemon',root,environment,workspace);await start();},
      close:async()=>{await stop(daemon);await stop(mock);await saveLogs(root);await rm(root,{recursive:true});},
    };
  } catch (error) {
    await Promise.all(children.map(stop)); await rm(root,{recursive:true});
    throw new Error('Could not start isolated live fixture',{cause:error});
  }
}

/** @param {number} port */
function providerConfig(port) {
  return `[providers.mock]\nname = "Mock (local)"\nprotocol = "openai-chat-completions"\nbase_url = "http://127.0.0.1:${port}/v1"\n[providers.mock.auth]\nnone = true\n[[providers.mock.models]]\nid = "mock-1"\nname = "Mock model"\n[providers.mock.models.capabilities]\ncontext_window = 32768\nmax_output_tokens = 4096\ntools = true\n[[providers.mock.models]]\nid = "mock-2"\nname = "Mock alternate"\n[providers.mock.models.capabilities]\ncontext_window = 32768\nmax_output_tokens = 4096\ntools = true\n`;
}

/** @param {string} command @param {string[]} args @param {string} name @param {string} root @param {NodeJS.ProcessEnv} env @param {string} cwd */
function launch(command,args,name,root,env,cwd) {
  const logPath = path.join(root,`${name}.log`);
  const log = openSync(logPath,'w');
  const child = spawn(command,args,{cwd,env,stdio:['ignore',log,log]});
  closeSync(log);
  child.on('error',error=>appendFileSync(logPath,`Launch failed: ${error.message}\n`));
  return child;
}

/** @param {string} root */
async function saveLogs(root) {
  const output = path.join(web,'test-results','live'); await mkdir(output,{recursive:true});
  await Promise.all(['daemon','mock'].map(name=>copyFile(path.join(root,`${name}.log`),path.join(output,`${name}.log`))));
}
