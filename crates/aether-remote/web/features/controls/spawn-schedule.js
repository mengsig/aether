import {el,invariant} from '../../ui/dom.js';
import {form} from './ui.js';
import {modelReference,required,integer} from './protocol.js';

/** @param {Record<string,string>} values @returns {import('../../types.js').Command} */
export function spawnScheduleCommand(values) {
  const workspace = required(values.workspace,'Workspace');
  invariant(workspace.startsWith('/'),'Use an absolute host workspace path.');
  invariant(['low','medium','high','x_high','max'].includes(values.effort),'Choose a valid effort.');
  return {cmd:'create_schedule',spec:{name:required(values.name,'Schedule name'),
    recurrence:{kind:'interval',seconds:integer(values.seconds,'Interval seconds')},busy:'skip',missed:'skip',
    max_deliveries:integer(values.limit,'Maximum runs'),
    spawn:{role:'worker',objective:required(values.objective,'Objective'),model:modelReference(values.model),
      scope:{workspace_root:workspace},config:{effort:values.effort},approval:{require_for:['external']}}}};
}

/** @param {import('./ui.js').Panel} view @param {()=>Promise<void>} refresh */
export function spawnScheduleForm(view,refresh) {
  const root = el('details','card');
  root.append(el('summary','','Schedule new runs'));
  root.append(el('p','muted','Each occurrence starts a fresh root agent. It has its own run and asks before external side effects.'));
  root.append(form(view,'Create scheduled run',[
    {label:'New-run schedule name',name:'name',required:true},
    {label:'Objective for each run',name:'objective',type:'textarea',required:true},
    {label:'Host workspace',name:'workspace',required:true},
    {label:'Provider/model',name:'model',required:true},
    {label:'Reasoning effort',name:'effort',options:['medium','low','high','x_high','max']},
    {label:'Interval in seconds',name:'seconds',type:'number',value:'86400',min:60,hint:'The daemon refuses intervals under 60 seconds.'},
    {label:'Maximum runs',name:'limit',type:'number',value:'7'},
  ],async values => {
    const command = spawnScheduleCommand(values);
    const result = await view.change(command,`Start a fresh agent in ${values.workspace} every ${values.seconds} seconds, for up to ${values.limit} runs?`);
    if (result) await refresh();
  }));
  return root;
}
