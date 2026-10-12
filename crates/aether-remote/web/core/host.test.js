import test from 'node:test';
import assert from 'node:assert/strict';
import {gitStatus,gitLabel,listWorkspaces,searchFiles} from './host.js';
import {hostPath} from './permissions.js';
import {assertAllowed} from './contract.js';

test('workspace existence and partial file completion remain explicit',async()=>{
  const transport={request:async command=>({data:command.cmd==='list_workspaces'?[{path:'/work/deleted',exists:false}]:{entries:[{path:'README.md',is_dir:false}],truncated:true}})};
  assert.equal((await listWorkspaces(transport))[0].exists,false);
  const files=await searchFiles(transport,'/work/new-repository','');
  assert.equal(files.truncated,true);assert.equal(files.entries[0].path,'README.md');
  await assert.rejects(listWorkspaces({request:async()=>({data:[{path:'/work/unknown'}]})}),/existence/);
});
test('git unavailable is distinct from a non-repository and detached HEAD retains changes',async()=>{
  const unavailable=await gitStatus({request:async()=>{throw Object.assign(new Error('Content filters'),{code:'unavailable'});}},'/work/repo');
  assert.match(gitLabel(unavailable),/Git status unavailable/);
  const detached=await gitStatus({request:async()=>({data:{repository:true,detached:true,head:'abc123',dirty:true,staged:0,modified:2}})},'/work/repo');
  assert.match(gitLabel(detached),/Detached HEAD abc123/);assert.match(gitLabel(detached),/0 staged.*2 modified/);
  assert.equal(detached.counts.untracked,null);
  await assert.rejects(gitStatus({request:async()=>{throw Object.assign(new Error('No access'),{code:'forbidden'});}},'/work/repo'),/No access/);
});
test('host paths validate bytes, controls and positive completion limits',()=>{
  assert.equal(hostPath('é'.repeat(2048)).length,2048);assert.throws(()=>hostPath('é'.repeat(2049)),/4096/);
  assert.throws(()=>hostPath('/work\nsecret'),/control/);assert.equal(hostPath('',true),'');
  assert.throws(()=>assertAllowed({cmd:'complete_path',prefix:'/',limit:0},'operator'),/positive/);
  assert.doesNotThrow(()=>assertAllowed({cmd:'complete_path',prefix:'',limit:50},'operator'));
});
test('remote configuration cannot grant host-only fields or spoof a trigger source',()=>{
  for(const patch of [{backend:null},{tools:{}}]) assert.throws(()=>assertAllowed({cmd:'update_agent',patch},'operator'),/host/);
  for(const scope of [{allowed_paths:[]},{repository:null}]) {
    assert.throws(()=>assertAllowed({cmd:'create_agent',scope},'operator'),/host/);
    assert.throws(()=>assertAllowed({cmd:'create_schedule',spec:{spawn:{scope}}},'operator'),/host/);
  }
  assert.doesNotThrow(()=>assertAllowed({cmd:'update_agent',patch:{instructions:'Review carefully'}},'operator'));
  for(const source of ['', 'operator\nmessage', 'a'.repeat(129),'é']) assert.throws(()=>assertAllowed({cmd:'trigger',source},'operator'),/Trigger/);
  assert.doesNotThrow(()=>assertAllowed({cmd:'trigger',source:'phone.review-1'},'operator'));
});
