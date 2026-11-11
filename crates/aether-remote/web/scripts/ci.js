import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {constants,existsSync} from 'node:fs';
import {access,mkdir,stat} from 'node:fs/promises';
import path from 'node:path';

const root = path.resolve(import.meta.dirname,'..');
const binary = process.env.AETHER_BINARY ? path.resolve(process.env.AETHER_BINARY) : undefined;
process.env.TMPDIR = path.join(root,'node_modules','.ci-tmp');
process.env.PLAYWRIGHT_BROWSERS_PATH = path.join(root,'node_modules','.browsers');
process.env.npm_config_cache = path.join(root,'node_modules','.npm-cache');
await mkdir(process.env.TMPDIR,{recursive:true});

/** A skipped browser check must be impossible to miss in the output. @param {string} reason */
function skipLoudly(reason) {
  console.log(`\n\x1b[1;33m==> Web browser checks SKIPPED, so the live web suite did not run: ${reason}\n    Set it up as crates/aether-remote/web/README.md describes; hosted CI runs it.\x1b[0m\n`);
}

/** @param {string} command @param {string[]} args */
function run(command,args) {
  assert.ok(command.length > 0,'A CI command must have an executable.');
  assert.ok(args.length > 0,'A CI command must specify its check.');
  const result = spawnSync(command,args,{cwd:root,stdio:'inherit',env:process.env});
  if (result.error) throw result.error;
  if (result.signal) throw new Error(`${command} terminated by ${result.signal}.`);
  assert.notEqual(result.status,null,'A completed CI command must have an exit status.');
  if (result.status !== 0) process.exit(result.status ?? 1);
}

async function browserAvailable() {
  const cli = path.join(root,'node_modules','@playwright','test','cli.js');
  if (!existsSync(cli)) {
    skipLoudly('needs local @playwright/test (npm ci in crates/aether-remote/web).');
    return false;
  }
  const {chromium} = await import('@playwright/test');
  let browser;
  try {
    browser = await chromium.launch({headless:true});
  } catch (error) {
    if (!(error instanceof Error) || !error.message.includes("Executable doesn't exist at ")) throw error;
    skipLoudly('needs Chromium in web/node_modules/.browsers (PLAYWRIGHT_BROWSERS_PATH=$PWD/node_modules/.browsers npx playwright install chromium).');
    return false;
  }
  try {
    assert.ok(browser.isConnected(),'The browser preflight must connect.');
    assert.equal(browser.contexts().length,0,'The browser preflight must start without contexts.');
  } finally { await browser.close(); }
  return true;
}

/** @param {string|undefined} binary */
async function runLive(binary) {
  if (!binary) {
    skipLoudly('set AETHER_BINARY to an executable daemon.');
    return;
  }
  assert.ok(path.isAbsolute(binary),'The daemon path must survive the web working directory change.');
  try {
    await access(binary,constants.X_OK);
    if (!(await stat(binary)).isFile()) throw new Error(`The daemon binary is not a file: ${binary}`);
  } catch (error) {
    if (!(error instanceof Error) || !('code' in error) || !['ENOENT','EACCES','ENOTDIR'].includes(String(error.code))) throw error;
    skipLoudly(`needs an executable daemon: ${binary}.`);
    return;
  }
  const python = spawnSync('python3',['--version'],{env:process.env,encoding:'utf8'});
  if (python.error && 'code' in python.error && python.error.code === 'ENOENT') {
    skipLoudly('the live suite needs python3.');
    return;
  }
  if (python.error) throw python.error;
  assert.equal(python.signal,null,'The Python preflight must not terminate by signal.');
  assert.equal(python.status,0,`Python preflight failed: ${python.stderr}`);
  run('npm',['run','test:live','--',binary]);
}

run(process.execPath,['scripts/precache.js','--check']);
run('npm',['run','typecheck']);
const browser = await browserAvailable();
process.env.RUN_CONTROLS_BROWSER = browser ? '1' : '0';
run('npm',['run','test:unit']);
if (browser) {
  run('npm',['run','test:browser']);
  await runLive(binary);
}
