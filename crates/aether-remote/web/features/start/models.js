import {invariant} from '../../ui/dom.js';

/** @param {unknown} value */
export function modelOptions(value) {
  invariant(Array.isArray(value), 'The daemon returned an invalid model list');
  return value.map(item => {
    invariant(item && typeof item === 'object', 'Invalid model record');
    const model = item.model;
    invariant(model && typeof model.provider === 'string' && typeof model.model === 'string', 'Invalid model identity');
    return {id:`${model.provider}/${model.model}`,label:typeof item.name === 'string' ? item.name : model.model,
      configured:item.configured === true,default:item.is_default === true};
  });
}

/** @param {{objective:string,name:string,workspace:string,model:string,effort:string,approval:string,task:boolean,owner:string}} input */
export function startCommand(input) {
  if (!input.objective.trim()) throw new Error('Describe the work to start.');
  if (input.task) {
    if (!input.owner) throw new Error('Choose an agent to own this task.');
    return {cmd:'create_task',objective:input.objective.trim(),owner:input.owner,priority:'normal'};
  }
  if (!input.workspace.startsWith('/')) throw new Error('Choose an absolute host workspace path.');
  const split = input.model.indexOf('/');
  if (split < 1 || split === input.model.length-1) throw new Error('Choose an available model.');
  invariant(['low','medium','high','x_high','max'].includes(input.effort), 'Invalid effort selection');
  invariant(['ask','yolo','writes'].includes(input.approval), 'Invalid approval selection');
  return {cmd:'create_agent',name:input.name.trim() || null,role:'worker',objective:input.objective.trim(),start:true,
    scope:{workspace_root:input.workspace},model:{provider:input.model.slice(0,split),model:input.model.slice(split+1)},
    approval:{require_for:input.approval === 'yolo' ? [] : input.approval === 'writes' ? ['external','mutation','idempotent_mutation'] : ['external']},config:{effort:input.effort}};
}
