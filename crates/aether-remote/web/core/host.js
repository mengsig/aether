import { array, object, string, integer } from './validate.js';
import { hostPath } from './permissions.js';
/** @typedef {import('../types.js').Transport} Transport */

/** @param {Transport} transport */
export async function listWorkspaces(transport) {
  const reply = await transport.request({cmd:'list_workspaces'});
  return array(reply.data).map(value => {
    const entry = object(value); const path = hostPath(entry.path);
    if (typeof entry.exists !== 'boolean') throw new Error('Workspace existence was not reported');
    return {path,name:path.split('/').filter(Boolean).at(-1) ?? path,exists:entry.exists};
  });
}

/** @param {unknown} value */
function completions(value) {
  const data = object(value);
  if (typeof data.truncated !== 'boolean') throw new Error('Completion coverage was not reported');
  const entries = array(data.entries).map(value => {
    const entry = object(value); const path = hostPath(entry.path);
    if (typeof entry.is_dir !== 'boolean') throw new Error('Missing directory entry type');
    return {path,name:path.replace(/\/$/,'').split('/').at(-1) ?? path,isDirectory:entry.is_dir};
  });
  return {entries,truncated:data.truncated};
}

/** @param {Transport} transport @param {string} path */
export async function listDirectory(transport,path) {
  const reply = await transport.request({cmd:'complete_path',kind:'directory',prefix:hostPath(path,true),limit:50});
  return completions(reply.data);
}

/** @param {Transport} transport @param {string} path @param {string} query */
export async function searchFiles(transport,path,query) {
  const reply = await transport.request({cmd:'complete_path',kind:'workspace_file',workspace:hostPath(path),prefix:hostPath(query,true),limit:50});
  return completions(reply.data);
}

/** @param {Transport} transport @param {string} path */
export async function gitStatus(transport,path) {
  hostPath(path);
  let reply;
  try { reply = await transport.request({cmd:'get_git_summary',workspace:path}); }
  catch (error) {
    if (error instanceof Error && 'code' in error && error.code === 'unavailable') return {available:false,repository:false,branch:null,head:null,root:null,dirty:null,detached:false,counts:{}};
    throw error;
  }
  const data = object(reply.data);
  if (typeof data.repository !== 'boolean' || typeof data.dirty !== 'boolean') throw new Error('Invalid Git repository state');
  const counts = Object.fromEntries(['ahead','behind','staged','modified','untracked'].map(key => [key,data[key] == null ? null : integer(data[key])]));
  return {available:true,repository:data.repository,branch:data.branch == null ? null : string(data.branch),head:data.head == null ? null : string(data.head),
    root:data.root == null ? null : hostPath(data.root),dirty:data.repository ? data.dirty : null,detached:data.detached === true,counts};
}

/** @param {Awaited<ReturnType<typeof gitStatus>>} git */
export function gitLabel(git) {
  if (!git.available) return 'Git status unavailable. You can still use a valid workspace.';
  if (!git.repository) return 'Not a Git repository';
  const state = git.dirty ? 'Uncommitted changes' : 'Working tree clean';
  const counts = Object.entries(git.counts).filter(([,value]) => value !== null).map(([key,value]) => `${value} ${key}`);
  return [git.detached ? `Detached HEAD${git.head ? ` ${git.head}` : ''}` : git.branch || 'Branch not reported',state,...counts].join(' · ');
}
