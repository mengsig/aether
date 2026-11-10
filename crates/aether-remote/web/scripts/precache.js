import {readdir,readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import path from 'node:path';
const root = path.resolve(import.meta.dirname,'..');
const directories = ['ui','core','features','assets'];
const files = ['index.html','boot.js','app.js','types.js','styles.css','manifest.webmanifest','push-renewal.js'];
/** @param {string} directory */
async function collect(directory) {
  const entries = await readdir(path.join(root,directory),{withFileTypes:true});
  for (const entry of entries) {
    const name = `${directory}/${entry.name}`;
    if (entry.isDirectory()) await collect(name);
    else if (/\.(?:js|css|svg|png)$/.test(name) && !name.endsWith('.test.js')) files.push(name);
  }
}
for (const directory of directories) await collect(directory);
files.sort();
const hash = createHash('sha256');
for (const file of files) { hash.update(file); hash.update(await readFile(path.join(root,file))); }
const source = `export const VERSION = ${JSON.stringify(hash.digest('hex').slice(0,16))};\nexport const PRECACHE = ${JSON.stringify(files.map(file=>`./${file}`),null,2)};\n`;
if (process.argv.includes('--check')) {
  if (await readFile(path.join(root,'precache.js'),'utf8') !== source) throw new Error('Precache is stale. Run npm run precache and commit precache.js.');
} else await writeFile(path.join(root,'precache.js'),source);
