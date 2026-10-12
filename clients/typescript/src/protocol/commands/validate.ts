// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Whether an agent can be started in `path`, and its canonical form.
 */
export interface ValidateWorkspaceCommand {
  "cmd": "validate_workspace";
  "path": string;
}

export function validateWorkspace(args: Omit<ValidateWorkspaceCommand, "cmd">): ValidateWorkspaceCommand {
  return { cmd: "validate_workspace", ...args };
}
