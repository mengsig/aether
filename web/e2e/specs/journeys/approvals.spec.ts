// Journey 3, approvals: an agent's tool call waits for a person, Inbox approves it and the agent
// goes on; then a standing rule is added from the agent's Controls.
//
// The faux engine's shell call waits. A real engine runs its own shell under the user's native
// permission settings, so the call that waits is an Aether tool (`complete_task`): the engine's
// permission prompt is one approval and the call itself another, each answered from the Inbox
// until the agent has finished.

import assert from "node:assert/strict";
import { expect, test } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { AGENT_LIMITS, REAL_MODELS } from "../../harness/real-engine.ts";
import { agent, approvalDecision, approvalRuleTools, assistantText, cancelLive, createAgentOn, pendingApprovals, toolCalls, toolResults } from "./daemon.ts";
import { connectionsReady, journeyHooks, type Journey } from "./journey.ts";
import { APPROVED_REPLY, OBJECTIVES, REAL_OBJECTIVES } from "./scripts.ts";
import { shootWindow } from "./capture.ts";
import { openControls } from "./steps.ts";
import { choose, confirmIfAsked, need, open, textField } from "./ui.ts";

const journey = journeyHooks({ archon: false });
const EVERY_EFFECT = ["read_only", "idempotent_mutation", "mutation", "external"] as const;
const WAIT_MS = { faux: 30_000, real: 180_000 } as const;
/** Each approval the real engine asks for costs it a turn, so its asks are bounded by its turn cap. */
const REAL_ASKS = AGENT_LIMITS.max_turns;
const FINISHED = new Set(["completed", "failed", "cancelled"]);
let agentId: string | undefined;

async function startAsking(run: Journey): Promise<string> {
  const real = run.variant === "real";
  return createAgentOn(run.stack, {
    name: "Journey approver", objective: real ? REAL_OBJECTIVES.approval : OBJECTIVES.approval, requireFor: EVERY_EFFECT,
    model: real ? { provider: REAL_MODELS[0].provider, model: REAL_MODELS[0].model } : { provider: "mock", model: "mock-1" },
    ...(real ? { limits: AGENT_LIMITS } : {}),
  });
}

/** Approves the first waiting call the Inbox shows; the window is already on the Inbox after the first time. */
async function approveFromInbox(run: Journey, first: boolean): Promise<void> {
  if (first) await open(run.window, "Inbox", "#/inbox");
  const approve = run.window.getByRole("button", { name: "Approve", exact: true }).first();
  await expect(approve, "Inbox offers to approve").toBeVisible({ timeout: WAIT_MS[run.variant] });
  if (first) await shootWindow(run.window, "inbox", "pending-approval");
  await approve.click();
  await confirmIfAsked(run.window, "Approve");
}

type Progress = "finished" | "asking" | "working";

async function progressOf(run: Journey, id: string): Promise<Progress> {
  const [pending, view] = await Promise.all([pendingApprovals(run.stack, id), agent(run.stack, id)]);
  if (pending.length > 0) return "asking";
  return FINISHED.has(view.state) ? "finished" : "working";
}

/** The real engine: every further ask for its call is approved, until the agent has finished. */
async function approveUntilFinished(run: Journey, id: string): Promise<void> {
  assert.equal(run.variant, "real", "only a real engine asks more than once");
  for (let ask = 2; ask <= REAL_ASKS; ask += 1) {
    const progress = await until(() => progressOf(run, id), (now) => now !== "working", `the agent to finish or to ask again (ask ${ask})`, WAIT_MS.real);
    if (progress === "finished") return;
    await approveFromInbox(run, false);
  }
  const progress = await until(() => progressOf(run, id), (now) => now !== "working", "the agent to finish after its last allowed ask", WAIT_MS.real);
  assert.equal(progress, "finished", `the agent finished within ${REAL_ASKS} asks`);
}

test("approvals journey: Inbox approves a waiting tool call and the agent goes on", async () => {
  const run = journey();
  await connectionsReady(run, false);
  const id = await startAsking(run);
  agentId = id;
  const pending = await until(() => pendingApprovals(run.stack, id), (rows) => rows.length > 0, "the tool call's approval", WAIT_MS[run.variant]);
  assert.equal(pending.length, 1, "one tool call waits");
  const first = pending[0]?.id;
  assert.ok(first !== undefined, "the waiting call has an id");
  await approveFromInbox(run, true);
  await until(() => pendingApprovals(run.stack, id), (rows) => rows.every((row) => row.id !== first), "the approval to be answered", WAIT_MS[run.variant]);
  assert.equal(await approvalDecision(run.stack, first), "approved", "the Inbox's Approve approved the call");
  if (run.variant === "real") {
    await approveUntilFinished(run, id);
    assert.equal((await agent(run.stack, id)).state, "completed", "the agent finished through its approved call");
    const finishes = (await toolCalls(run.stack, id)).filter((call) => call.name.endsWith("complete_task"));
    assert.ok(finishes.some((call) => call.result !== null && !call.result.failed), `the approved complete_task call ran and answered: ${JSON.stringify(finishes)}`);
    return;
  }
  await until(() => assistantText(run.stack, id), (said) => said.includes(APPROVED_REPLY), "the agent to go on", WAIT_MS.faux);
  const results = await toolResults(run.stack, id);
  assert.ok(results.some((result) => !result.failed && result.text.includes("approval-probe")), `the approved call ran and printed its probe: ${JSON.stringify(results)}`);
});

test("approvals journey: a standing rule added from Controls", async () => {
  const run = journey();
  assert.ok(agentId !== undefined, "the first approvals test started the agent");
  try {
    const controls = await openControls(run.window, run.stack, agentId, "Approvals");
    await choose(run.window, await textField(await need(controls, "rule-tool")), "bash");
    const submit = await need(controls, "rule-submit");
    await expect(submit, "the rule is ready to add").toBeEnabled();
    await shootWindow(run.window, "controls", "approvals-rule");
    await submit.click();
    await confirmIfAsked(run.window, "Add rule");
    const tools = await until(() => approvalRuleTools(run.stack), (now) => now.includes("bash"), "the rule for bash", 30_000);
    assert.equal(tools.filter((tool) => tool === "bash").length, 1, "one rule for bash was added");
  } finally {
    await cancelLive(run.stack, [agentId]);
  }
});
