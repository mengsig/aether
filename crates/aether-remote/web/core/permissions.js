import { object, string } from './validate.js';

/** @param {unknown} value @param {boolean} [allowEmpty] */
export function hostPath(value, allowEmpty = false) {
  if (typeof value !== 'string' || (!allowEmpty && !value)) throw new Error('Host path must be text and cannot be empty');
  if (new TextEncoder().encode(value).length > 4096 || /[\x00-\x1f\x7f-\x9f]/.test(value)) throw new Error('Host paths must fit 4096 bytes and contain no control characters');
  return value;
}

/** @param {import('../types.js').Command} command */
export function assertRemoteFields(command) {
  string(command.cmd);
  if (command.cmd === 'trigger' && (typeof command.source !== 'string' || !/^[A-Za-z0-9._-]{1,128}$/.test(command.source))) throw new Error('Trigger source must be 1–128 ASCII letters, digits, dots, underscores or hyphens');
  if (command.cmd === 'update_agent') {
    const patch = object(command.patch);
    if (Object.hasOwn(patch,'backend') || Object.hasOwn(patch,'tools')) throw new Error('Backend and tool changes are available only on the daemon host');
  }
  const spec = command.cmd === 'create_agent' ? command : command.cmd === 'create_schedule' && command.spec && object(command.spec).spawn ? object(object(command.spec).spawn) : null;
  if (spec?.scope) {
    const scope = object(spec.scope);
    if (Object.hasOwn(scope,'allowed_paths') || Object.hasOwn(scope,'repository')) throw new Error('Workspace grants and repository identity are managed on the daemon host');
  }
  if (command.cmd === 'complete_path') {
    hostPath(command.prefix,true);
    if (command.workspace != null) hostPath(command.workspace);
    if (command.limit != null && (!Number.isSafeInteger(command.limit) || Number(command.limit) < 1)) throw new Error('Completion limit must be a positive integer');
  }
  if (command.cmd === 'get_git_summary') hostPath(command.workspace);
  if (command.cmd === 'validate_workspace') hostPath(command.path);
}
