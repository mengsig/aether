// The faux engine's replies for the journeys, keyed on the objective or on the tools a request
// advertises: a shell call for first-launch and approvals, and Noesis's Prober and Practitioner.

import { DEFAULT_REPLY, type EngineRequest, type Reply } from "../../harness/engine.ts";

/**
 * First launch messages, stops and resumes its agent while it still works: a worker completes once
 * it answers, and then reads no more messages. So it waits in a shell call, at most HOLD_SECONDS,
 * until the journey drops this file in its workspace.
 */
export const RELEASE_FILE = ".journey-release";
const HOLD_SECONDS = 100;
const HOLD = `for i in $(seq ${HOLD_SECONDS}); do [ -e ${RELEASE_FILE} ] && exit 0; sleep 1; done; exit 1`;

/** Objectives the journeys type; the faux engine recognises them. */
export const OBJECTIVES = {
  firstLaunch: "List files in the workspace, then finish.",
  approval: "Run the approval probe, then finish.",
} as const;
/** The real variant's objectives: one cheap shell call, then one word. */
export const REAL_OBJECTIVES = {
  firstLaunch: `Run \`echo pong\` with your shell tool and say pong. Then run \`${HOLD}\` with your shell tool in the foreground, and finish.`,
  approval: "Run `echo approval-probe` with your shell tool, then reply with the word done and finish.",
} as const;
/** The shell commands each journey's agent runs, faux and real alike. */
export const FAUX_COMMANDS = { firstLaunch: "ls -la .", hold: HOLD, approval: "echo approval-probe" } as const;
export const REAL_COMMANDS = { firstLaunch: "echo pong", hold: HOLD, approval: "echo approval-probe" } as const;
export const LISTED_REPLY = DEFAULT_REPLY;
export const APPROVED_REPLY = "Approval probe done.";
/** The notice the daemon sends the agent, as its own user message, once a person approves a call. */
const APPROVAL_NOTICE = /^\[inbox \d+\] Approval apr_\S+ approved by /;
export const PROBER_QUESTION = "Before we start: what does 2x mean?";
export const PRACTICE_STATEMENT = "Solve 2x = 8";

/** The advertised tool whose name ends with `suffix` (the daemon writes `noesis.ask` as `noesis_ask`). */
function tool(request: EngineRequest, suffix: string): string | undefined {
  return request.toolNames.find((name) => name === suffix || name.endsWith(`_${suffix}`) || name.endsWith(`.${suffix}`));
}

function shellCall(request: EngineRequest, command: string, text: string): Reply {
  const shell = tool(request, "bash") ?? "bash";
  return { toolCall: { name: shell, arguments: { command }, text } };
}

/** The Prober asks one recall question, then hands over; the Teacher plans one step and asks it. */
function noesisReply(request: EngineRequest): Reply | undefined {
  const answered = request.toolResults.length > 0;
  const probeDone = tool(request, "probe_done");
  const ask = tool(request, "ask");
  if (probeDone !== undefined && ask !== undefined) {
    if (!answered) return { toolCall: { name: ask, arguments: { kind: "recall", prompt: PROBER_QUESTION, answer_key: "two times x", explanation: "2x is 2 multiplied by x." } } };
    return { toolCall: { name: probeDone, arguments: { strands: [{ strand: "algebra", floor: "knows arithmetic", ceiling: "linear equations", misconceptions: [], start: "solving for x" }] } } };
  }
  const plan = tool(request, "plan");
  if (plan !== undefined && ask !== undefined) {
    if (request.toolResults.length === 0) return { toolCall: { name: plan, arguments: { nodes: [{ id: "solve", title: "Solving for x" }], edges: [] } } };
    if (request.toolResults.length === 1) return { toolCall: { name: ask, arguments: { kind: "recall", prompt: `Practice: ${PRACTICE_STATEMENT}`, answer_key: "x = 4", explanation: "Halve both sides." } } };
    return "Good; on to the next step.";
  }
  const problem = tool(request, "problem");
  if (problem !== undefined) {
    if (!answered) return { toolCall: { name: problem, arguments: { statement: PRACTICE_STATEMENT, mode: "recall", solution: "x = 4 by halving" }, text: "Here is a problem." } };
    return "Take your time.";
  }
  return undefined;
}

/** Lists the workspace, answers while holding in a shell call, then finishes once released. */
function firstLaunchReply(request: EngineRequest): Reply {
  const done = request.toolResults.length;
  if (done === 0) return shellCall(request, FAUX_COMMANDS.firstLaunch, "Listing the workspace.");
  if (done === 1) return shellCall(request, FAUX_COMMANDS.hold, LISTED_REPLY);
  return LISTED_REPLY;
}

export function journeyScript(request: EngineRequest): Reply {
  const noesis = noesisReply(request);
  if (noesis !== undefined) return noesis;
  const answered = request.toolResults.length > 0;
  if (request.lastUserText.includes(OBJECTIVES.firstLaunch)) return firstLaunchReply(request);
  if (request.lastUserText.includes(OBJECTIVES.approval)) return answered ? APPROVED_REPLY : shellCall(request, FAUX_COMMANDS.approval, "Running the probe.");
  if (APPROVAL_NOTICE.test(request.lastUserText)) return APPROVED_REPLY;
  return DEFAULT_REPLY;
}
