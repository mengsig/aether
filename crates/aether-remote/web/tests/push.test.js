import test from 'node:test';
import assert from 'node:assert/strict';
import {notificationData,safeNotificationRoute} from '../ui/push.js';
import {assertProtocol} from '../dev/validate.js';

const agent = 'agt_00000000000000000000000001';
const approval = 'apr_00000000000000000000000001';
const base = {v:1,instance_id:'host',seq:9,at:'2026-10-12T00:00:00Z',title:'Needs you',agent};

test('all seven notification kinds use the exact required daemon fragment',()=>{
  const variants = [
    {kind:'approval',approval,url:`#attention?item=${approval}`},
    {kind:'question',question_seq:9,url:'#attention?item=question-9'},
    ...['agent_failed','agent_blocked','task_completed','task_failed'].map(kind=>({kind,url:`#agent/${agent}`})),
    {kind:'test',seq:0,url:'#activity'},
  ];
  for(const variant of variants) {
    const payload={...base,...variant}; assertProtocol('PushPayload',payload);
    assert.equal(notificationData(payload,'host').url,payload.url);
    assert.equal(safeNotificationRoute(payload.url),payload.url);
  }
  assert.equal(notificationData({...base,agent:undefined,kind:'task_completed',url:'#activity'},'host').url,'#activity');
});
test('notification routes reject missing, external, malformed and inconsistent targets',()=>{
  const payload={...base,kind:'approval',approval,url:`#attention?item=${approval}`};
  assert.throws(()=>notificationData(payload,'other'),/different daemon/);
  assert.throws(()=>notificationData({...payload,url:undefined},'host'),/deep link/);
  assert.throws(()=>assertProtocol('PushPayload',{...payload,url:undefined}),/Invalid/);
  for(const url of ['https://evil.example','#fleet','#attention','#attention?item=question-9007199254740992',`#agent/${agent}?redirect=https://evil.example`,'#agent/agt_short',`#agent/${agent}\n`]) {
    assert.equal(safeNotificationRoute(url),'#activity');
    assert.throws(()=>notificationData({...payload,url},'host'),/deep link/);
  }
  assert.throws(()=>notificationData({...payload,url:`#agent/${agent}`},'host'),/deep link/);
});
test('notification tags coalesce per agent and class while previews preserve Unicode bounds',()=>{
  const question=notificationData({...base,kind:'question',question_seq:9,url:'#attention?item=question-9',agent_name:'A'.repeat(90),preview:'🚀'.repeat(220)},'host');
  const request=notificationData({...base,kind:'approval',approval,url:`#attention?item=${approval}`,seq:10},'host');
  const status=notificationData({...base,kind:'agent_failed',url:`#agent/${agent}`},'host');
  assert.equal(question.tag,request.tag);assert.notEqual(question.tag,status.tag);
  assert.equal(Array.from(question.body.split(' · ')[0]).length,80);
  assert.equal(Array.from(question.body.split(' · ')[1]).length,200);
});
