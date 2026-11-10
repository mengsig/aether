import {el,field,heading,errorText,empty,invariant} from '../../ui/dom.js';
import {startCommand} from './models.js';
import {workspacePicker} from './workspace.js';
import {segments,modelPicker} from './segments.js';

/** @param {HTMLElement} container @param {import('../../types.js').Context} context */
export function mountNew(container,context) {
  container.append(heading('Start something.','One clear objective. A fresh pair of hands.'));
  if (context.store.get().role === 'observer') { container.append(empty('Observer access','An operator device can start new work.')); return () => {}; }
  const query = new URLSearchParams(location.hash.split('?')[1] || '');
  const form = el('form','start-form stack');
  const objective = el('textarea','start-objective'); objective.required = true; objective.maxLength = 100000;
  objective.placeholder = 'What would you like to work on?';
  objective.value = query.get('panel') === 'workspace' ? context.store.getDraft('new') : query.get('value') || context.store.getDraft('new');
  objective.addEventListener('input',() => context.store.setDraft('new',objective.value));
  const error = el('p','error-banner'); error.hidden = true; error.setAttribute('role','alert');
  const showError = (/** @type {unknown} */ cause) => { error.textContent = errorText(cause); error.hidden = false; };
  const workspace = workspacePicker(context,query.get('panel') === 'workspace' ? query.get('value') || '' : '');
  const model = modelPicker(context);
  const effort = segments('Effort','start-effort',[
    {id:'low',label:'Low'},{id:'medium',label:'Medium'},{id:'high',label:'High'},{id:'x_high',label:'Extra high'},{id:'max',label:'Max'},
  ],'medium');
  effort.container.classList.add('start-effort');
  const agentFields = el('div','start-agent-fields stack'); agentFields.append(workspace.container,model.container,effort.container);
  const submit = el('button','button primary','Start agent'); submit.type = 'submit';
  const options = advancedOptions(context,task => { agentFields.hidden = task; submit.textContent = task ? 'Create task' : 'Start agent'; });
  const dock = el('div','start-action'); dock.append(submit);
  form.append(field('Objective',objective),agentFields,options.container,error,dock); container.append(form);
  const releaseDock = keepActionVisible(dock); let disposed = false;
  form.addEventListener('submit',async event => {
    event.preventDefault(); if (!form.reportValidity() || submit.disabled) return;
    submit.disabled = true; form.setAttribute('aria-busy','true'); error.hidden = true;
    try {
      invariant(context.store.get().role === 'operator','Only an operator can start work.');
      const task = options.mode.value === 'task';
      const input = {objective:objective.value,name:options.name.value,model:model.value,effort:effort.value,approval:options.approval.value,task,owner:options.owner.value};
      if (!task && !input.model) throw new Error('Choose a configured model before starting.');
      if (task && !context.store.get().agents.some(agent => agent.id === input.owner && !['completed','failed','cancelled'].includes(agent.status))) throw new Error('Choose an active agent to own this task.');
      const canonical = task ? '' : await workspace.validate();
      if (disposed) return;
      invariant(task || canonical.startsWith('/'),'Workspace must be canonical');
      const reply = await context.command(startCommand({...input,workspace:canonical}));
      if (context.store.getDraft('new') === input.objective) context.store.setDraft('new','');
      if (disposed) return;
      const data = /** @type {{id?:string}} */ (reply.data || {});
      context.toast(task ? 'Task created.' : 'Agent started.'); context.navigate(!task && data.id ? `agent/${data.id}` : 'fleet');
    } catch (cause) { if (!disposed) showError(cause); }
    finally { if (!disposed) { submit.disabled = false; form.removeAttribute('aria-busy'); } }
  });
  return () => { disposed = true; workspace.destroy(); model.destroy(); releaseDock(); };
}

/** @param {import('../../types.js').Context} context @param {(task:boolean)=>void} changeMode */
function advancedOptions(context,changeMode) {
  const container = el('details','start-options'); container.append(el('summary','','Options'));
  const body = el('div','stack start-options-body');
  const name = el('input'); name.placeholder = 'Optional'; name.maxLength = 128;
  const ownerChoices = context.store.get().agents.filter(agent => !['completed','failed','cancelled'].includes(agent.status)).map(agent => ({id:agent.id,label:agent.name}));
  const owner = segments('Task owner','start-owner',ownerChoices,ownerChoices[0]?.id || '');
  owner.container.hidden = true;
  const noOwners = el('p','muted small','No active agents. Start an agent first.'); noOwners.hidden = true;
  const approval = segments('Approval mode','start-approval',[
    {id:'ask',label:'External actions'},{id:'writes',label:'All changes'},{id:'yolo',label:'Auto approve'},
  ],'ask');
  const agentOptions = el('div','stack'); agentOptions.append(field('Name',name),approval.container);
  const mode = segments('Work type','start-type',[{id:'agent',label:'New agent'},{id:'task',label:'Existing agent task'}],'agent',value => {
    const task = value === 'task'; owner.container.hidden = !task; noOwners.hidden = !task || ownerChoices.length > 0;
    agentOptions.hidden = task; changeMode(task);
  });
  body.append(mode.container,agentOptions,owner.container,noOwners); container.append(body);
  return {container,name,mode,owner,approval};
}

/** @param {HTMLElement} dock */
function keepActionVisible(dock) {
  const viewport = window.visualViewport;
  const update = () => {
    const inset = viewport ? Math.max(0,window.innerHeight - viewport.height - viewport.offsetTop) : 0;
    dock.style.setProperty('--start-keyboard-inset',`${inset}px`);
    dock.classList.toggle('keyboard-open',inset > 100);
  };
  viewport?.addEventListener('resize',update); viewport?.addEventListener('scroll',update); update();
  return () => { viewport?.removeEventListener('resize',update); viewport?.removeEventListener('scroll',update); };
}
