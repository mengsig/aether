/** @typedef {{name:string,description:string,route:string,aliases?:string[]}} SlashCommand */
/** @type {SlashCommand[]} */
export const commands = [
  {name:'model', description:'Choose model and reasoning effort', route:'controls', aliases:['models']},
  {name:'effort', description:'Choose reasoning effort', route:'controls'},
  {name:'new', description:'Start a new root agent', route:'new'},
  {name:'workspace', description:'Choose a workspace for new work', route:'new', aliases:['cd','ws','workspaces']},
  {name:'agent', description:'Find an agent in the fleet', route:'fleet', aliases:['agents']},
  {name:'compact', description:'Fold context into a checkpoint', route:'controls', aliases:['checkpoint']},
  {name:'copy', description:'Copy the last agent reply', route:'local'},
  {name:'memory', description:'Browse, pin, or forget memories', route:'controls', aliases:['memories']},
  {name:'approvals', description:'Approval rules and auto-approve', route:'controls', aliases:['rules','yolo']},
  {name:'usage', description:'Provider health, sign-in, and account limits', route:'controls', aliases:['providers']},
  {name:'settings', description:'Theme and motion preferences', route:'settings', aliases:['theme','motion','mouse']},
  {name:'outbox', description:'Review unconfirmed deliveries', route:'settings'},
  {name:'remember', description:'Record a durable operator fact', route:'controls'},
  {name:'stop', description:'Interrupt this turn; preserve context', route:'controls', aliases:['interrupt']},
  {name:'cancel', description:'End this agent with confirmation', route:'controls', aliases:['kill']},
  {name:'pause', description:'Pause this agent or its subtree', route:'controls'},
  {name:'resume', description:'Resume paused work', route:'controls'},
  {name:'wake', description:'Wake a failed agent in place', route:'controls'},
  {name:'restart', description:'Restart from the last checkpoint', route:'controls'},
  {name:'rename', description:'Rename this agent', route:'controls'},
  {name:'instructions', description:'Replace standing instructions', route:'controls', aliases:['standing']},
  {name:'artifacts', description:'Browse and download agent artifacts', route:'controls'},
  {name:'mcp', description:'Inspect MCP servers and tools', route:'controls'},
  {name:'schedules', description:'Manage recurring timers', route:'controls'},
  {name:'report', description:'Agents, tools, cost, and open loops', route:'controls'},
  {name:'doctor', description:'Connection and provider diagnostics', route:'controls'},
  {name:'voice', description:'Use your phone keyboard’s microphone', route:'local'},
  {name:'login', description:'Manage provider sign-in', route:'controls'},
  {name:'logout', description:'Manage provider credentials', route:'controls'},
  {name:'clear', description:'Hide retained messages in this view', route:'local'},
  {name:'help', description:'Show commands and keyboard shortcuts', route:'local', aliases:['?']},
  {name:'quit', description:'Return to fleet; agents keep running', route:'fleet', aliases:['exit','q']},
  {name:'mode', description:'Transcript display preferences', route:'settings'},
  {name:'attention', description:'Questions and approvals needing you', route:'attention'},
  {name:'activity', description:'Fleet event timeline', route:'activity'},
  {name:'search', description:'Find text in this conversation', route:'local'},
];

/** @param {string} query */
export function findCommands(query) {
  const term = query.replace(/^\//, '').trim().toLocaleLowerCase();
  return commands.filter(command => `${command.name} ${command.description} ${(command.aliases ?? []).join(' ')}`.toLocaleLowerCase().includes(term));
}

/** @param {string} draft */
export function parseCommand(draft) {
  const match = /^\/([^\s]+)(?:\s+([\s\S]*))?$/.exec(draft.trim());
  if (!match) return null;
  const name = match[1].toLowerCase();
  const command = commands.find(item => item.name === name || item.aliases?.includes(name));
  return command ? {command, argument:match[2] ?? ''} : null;
}

/** @param {SlashCommand} command @param {string} agentId @param {string} argument */
export function commandRoute(command, agentId, argument) {
  const base = command.route === 'controls' ? `controls/${encodeURIComponent(agentId)}` : command.route;
  const panels = {memory:'memories', remember:'memories', approvals:'rules', report:'runs', usage:'diagnostics', mcp:'diagnostics', doctor:'diagnostics', login:'diagnostics', logout:'diagnostics'};
  const panel = command.route === 'controls' ? (/** @type {Record<string,string>} */(panels)[command.name] ?? (['artifacts','schedules'].includes(command.name) ? command.name : 'agent')) : command.name;
  const params = new URLSearchParams({panel});
  if (command.route === 'controls') params.set('action', command.name);
  if (argument) params.set('value', argument);
  return `${base}?${params}`;
}
