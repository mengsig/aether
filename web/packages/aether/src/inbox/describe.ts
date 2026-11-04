// Plain words for a submission waiting in the recovery review: what it would do, and to whom.

import { invariant } from "../fleet/invariant.ts";

type Json = Record<string, unknown>;

const VERBS: Readonly<Record<string, string>> = {
  send_message: "Message", reply_to_question: "Reply", resolve_approval: "Approval answer", cancel: "Cancel", interrupt: "Interrupt",
  pause: "Pause", resume: "Resume", restart: "Restart", archive: "Archive", update_agent: "Update", set_approvals: "Approval policy",
  add_approval_rule: "New rule", create_schedule: "New schedule", update_schedule: "Schedule change", create_agent: "Start agent",
  create_task: "New task", create_memory: "New memory", forget_memory: "Forget memory", pin_memory: "Pin memory",
};

/** The agent or record a command acts on, as named by its own fields. */
function target(command: Json): string | null {
  for (const field of ["agent", "to", "id"]) {
    if (typeof command[field] === "string") return command[field];
  }
  const nested = command.target;
  if (nested !== null && typeof nested === "object" && typeof (nested as Json).agent === "string") return (nested as Json).agent as string;
  return null;
}

/** One line for the review: "Cancel · release-captain", with the agent's name when known. */
export function describeCommand(command: Json, names: Readonly<Record<string, string>>): { readonly title: string; readonly detail: string | null } {
  invariant(typeof command.cmd === "string" && command.cmd !== "", "a submission is a named command");
  const verb = VERBS[command.cmd] ?? command.cmd.replaceAll("_", " ");
  const id = target(command);
  const title = id === null ? verb : `${verb} · ${names[id] ?? id}`;
  const text = typeof command.content === "string" ? command.content : typeof command.reason === "string" ? command.reason : null;
  return { title, detail: text === null ? null : text.length > 140 ? `${text.slice(0, 139)}…` : text };
}
