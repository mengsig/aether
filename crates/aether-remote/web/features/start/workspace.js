import {el,button,field,errorText,invariant} from '../../ui/dom.js';
import {listWorkspaces,listDirectory,gitStatus} from '../../core/contract.js';
import {gitLabel} from '../../core/host.js';
import {choiceSheet} from './segments.js';
import {icon} from '../../ui/icons.js';

/** @param {import('../../types.js').Context} context @param {string} initial */
export function workspacePicker(context,initial = '') {
  const container = el('section','start-workspace'); container.setAttribute('aria-label','Workspace');
  const sheet = choiceSheet('Choose a workspace');
  const input = el('input'); input.placeholder = '/path/on/your/host'; input.value = initial;
  input.autocomplete = 'off'; input.spellcheck = false; input.setAttribute('autocapitalize','off');
  const selected = button('Choose a folder',() => { sheet.open(); browser.browse(); },'button start-workspace-selected');
  const status = el('p','start-workspace-status muted small'); status.setAttribute('role','status');
  const recent = el('div','start-recents'); recent.setAttribute('aria-label','Recent workspaces');
  const label = el('div','start-workspace-heading');
  label.append(el('span','field-label','Workspace'),button('Browse',() => { sheet.open(); browser.browse(); },'button quiet'));
  let chosen = initial; let generation = 0; let disposed = false;
  /** @param {string} path */
  const choose = async path => {
    chosen = path; input.value = path; selected.replaceChildren(icon('folder'),el('span','',path || 'Choose a folder')); selected.title = path;
    selected.hidden = false;
    recent.querySelectorAll('button').forEach(node => node.setAttribute('aria-pressed',String(node.title === path)));
    const token = ++generation; status.textContent = 'Checking workspace…'; sheet.dialog.close();
    const message = await workspaceStatus(context,path);
    if (!disposed && token === generation) status.textContent = message;
  };
  const browser = workspaceBrowser(context,input,path => { void choose(path); });
  sheet.body.append(field('Host workspace',input),browser.container);
  container.append(label,selected,recent,status,sheet.dialog);
  listWorkspaces(context.transport).then(workspaces => {
    if (disposed) return;
    selected.hidden = !chosen && workspaces.some(workspace => workspace.exists);
    for (const workspace of workspaces.slice(0,3)) {
      const chip = button('',() => { void choose(workspace.path); },'button workspace-choice start-recent');
      chip.append(icon('folder'),el('span','',workspace.path + (workspace.exists ? '' : ' · Unavailable')));
      chip.title = workspace.path; chip.setAttribute('aria-label',workspace.path + (workspace.exists ? '' : ' · No longer exists'));
      chip.setAttribute('aria-pressed',String(chosen === workspace.path)); chip.disabled = !workspace.exists; recent.append(chip);
    }
  }).catch(cause => { if (!disposed) recent.append(el('p','error-banner',`Recent workspaces: ${errorText(cause)}`)); });
  if (initial) void choose(initial);
  return {container,input,validate:() => validateWorkspace(context,chosen),destroy:() => { disposed = true; browser.destroy(); sheet.destroy(); }};
}

/** @param {import('../../types.js').Context} context @param {HTMLInputElement} input @param {(path:string)=>void} choose */
function workspaceBrowser(context,input,choose) {
  const container = el('div','stack'); const results = el('div','start-directory-results');
  const status = el('p','muted small'); status.setAttribute('role','status');
  const preview = el('p','muted small'); preview.setAttribute('role','status');
  const use = button('Use this folder',() => choose(input.value),'button primary');
  let disposed = false; let generation = 0; let timer = 0;
  const browse = async () => {
    clearTimeout(timer); const token = ++generation; const path = input.value || '/';
    results.replaceChildren(); status.textContent = 'Finding host folders…'; preview.textContent = '';
    use.disabled = !input.value.startsWith('/');
    const checks = workspaceStatus(context,path).then(message => {
      if (!disposed && token === generation) preview.textContent = message;
    });
    try {
      const {entries,truncated} = await listDirectory(context.transport,path);
      if (disposed || token !== generation) return;
      const folders = entries.filter(entry => entry.isDirectory);
      status.textContent = truncated ? 'More folders match. Keep typing to narrow the list.' : folders.length ? 'Folders on your host' : 'No matching folders.';
      for (const entry of folders) {
        const row = el('div','start-folder-row');
        const pick = button(entry.name,() => choose(entry.path),'button workspace-choice');
        pick.setAttribute('aria-label',`Use ${entry.path}`); pick.title = entry.path;
        const deeper = button('Open',() => { input.value = entry.path.replace(/\/$/,'') + '/'; void browse(); },'button quiet');
        deeper.setAttribute('aria-label',`Browse ${entry.path}`); row.append(pick,deeper); results.append(row);
      }
    } catch (cause) { if (!disposed && token === generation) status.textContent = errorText(cause); }
    await checks;
  };
  input.addEventListener('input',() => {
    clearTimeout(timer); ++generation; results.replaceChildren(); status.textContent = 'Finding host folders…'; preview.textContent = '';
    use.disabled = !input.value.startsWith('/'); timer = window.setTimeout(() => { void browse(); },250);
  });
  input.addEventListener('keydown',event => { if (event.key === 'Enter') { event.preventDefault(); if (!use.disabled) choose(input.value); } });
  container.append(status,results,preview,use);
  return {container,browse:() => { void browse(); },destroy:() => { disposed = true; clearTimeout(timer); }};
}

/** @param {import('../../types.js').Context} context @param {string} path */
async function workspaceStatus(context,path) {
  const [validation,git] = await Promise.allSettled([validateWorkspace(context,path),gitStatus(context.transport,path)]);
  if (validation.status === 'rejected') return errorText(validation.reason);
  if (git.status === 'rejected') return `Workspace ready · Git status unavailable: ${errorText(git.reason)}`;
  return `Workspace ready · ${gitLabel(git.value)}`;
}

/** @param {import('../../types.js').Context} context @param {string} path */
async function validateWorkspace(context,path) {
  if (!path.startsWith('/')) throw new Error('Choose a workspace folder on your host.');
  const reply = await context.transport.request({cmd:'validate_workspace',path});
  invariant(reply.reply === 'workspace_validation' && reply.data && typeof reply.data === 'object','Invalid workspace validation reply');
  const data = /** @type {Record<string,unknown>} */ (reply.data);
  if (data.valid !== true) throw new Error(typeof data.reason === 'string' ? data.reason : 'This workspace is not available on the host.');
  invariant(typeof data.path === 'string' && data.path.startsWith('/'),'The daemon did not provide a canonical workspace');
  return data.path;
}
