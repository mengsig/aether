import {mkdir,readdir} from 'node:fs/promises';
import {spawn} from 'node:child_process';
import assert from 'node:assert/strict';
import path from 'node:path';
const controlsBrowser = process.env.RUN_CONTROLS_BROWSER ?? '1';
if (!['0','1'].includes(controlsBrowser)) throw new Error('RUN_CONTROLS_BROWSER must be 0 or 1.');
const root = path.resolve(import.meta.dirname,'..');
const temporary = path.join(root,'node_modules','.unit-tmp');
await mkdir(temporary,{recursive:true});
const directories = ['core','tests'];
for (const entry of await readdir(path.join(root,'features'),{withFileTypes:true})) {
  if (entry.isDirectory()) directories.push(`features/${entry.name}`);
}
directories.push('features/fleet/graph');
const files = [];
for (const directory of directories) {
  for (const file of await readdir(path.join(root,directory))) if (file.endsWith('.test.js')) files.push(`${directory}/${file}`);
}
assert.ok(files.length > 0,'Unit test discovery must find tests.');
assert.equal(new Set(files).size,files.length,'Unit test discovery must not duplicate tests.');
const child = spawn(process.execPath,['--test',...files],{cwd:root,stdio:'inherit',env:{...process.env,TMPDIR:temporary,
  PLAYWRIGHT_BROWSERS_PATH:path.join(root,'node_modules','.browsers'),RUN_CONTROLS_BROWSER:controlsBrowser,RUN_CONVERSATION_BROWSER:controlsBrowser},
});
child.on('error',error=> { console.error(error); process.exitCode=1; });
child.on('exit',code=> { process.exitCode=code ?? 1; });
