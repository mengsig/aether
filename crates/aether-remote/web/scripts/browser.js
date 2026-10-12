import {mkdir} from 'node:fs/promises';
import {spawn} from 'node:child_process';
import path from 'node:path';
const root = path.resolve(import.meta.dirname,'..');
const temporary = path.join(root,'node_modules','.browser-tmp');
await mkdir(temporary,{recursive:true});
const child = spawn(process.execPath,[path.join(root,'node_modules','@playwright','test','cli.js'),'test',...process.argv.slice(2)],{
  cwd:root,stdio:'inherit',env:{...process.env,TMPDIR:temporary,PLAYWRIGHT_BROWSERS_PATH:path.join(root,'node_modules','.browsers')},
});
child.on('error',error=> { console.error(error); process.exitCode=1; });
child.on('exit',code=> { process.exitCode=code ?? 1; });
