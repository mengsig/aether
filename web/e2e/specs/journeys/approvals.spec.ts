// Journey 3, approvals: an agent's tool call waits for a person, Inbox approves it and the agent
// goes on; then a standing rule is added from the agent's Controls.

import assert from "node:assert/strict";
import { expect, test } from "@playwright/test";
import { until } from "../../harness/process.ts";
import { AGENT_LIMITS, REAL_MODELS } from "../../harness/real-engine.ts";
import { approvalRuleTools, cancelLive, createAgentOn, pendingApprovals, transcriptText } from "./daemon.ts";
import { connectionsReady, journeyHooks, type Journey } from "./journey.ts";
import { APPROVED_REPLY, OBJECTIVES, REAL_OBJECTIVES } from "./scripts.ts";
import { openControls } from "./steps.ts";
import { choose, confirmIfAsked, need, open, textField } from "./ui.ts";

const journey = journeyHooks({ archon: false });
const EVERY_EFFECT = ["read_only", "idempotent_mutation", "mutation", "external"] as const;
const WAIT_MS = { faux: 30_000, real: 120_000 } as const;
let agentId: string | undefined;

async function startAsking(run: Journey): Promise<string> {
  const real = run.variant === "real";
  return createAgentOn(run.stack, {
    name: "Journey approver", objective: real ? REAL_OBJECTIVES.approval : OBJECTIVES.approval, requireFor: EVERY_EFFECT,
    model: real ? { provider: REAL_MODELS[0].provider, model: REAL_MODELS[0].model } : { provider: "mock", model: "mock-1" },
    ...(real ? { limits: AGENT_LIMITS } : {}),
  });
}

test("approvals journey: Inbox approves a waiting tool call and the agent goes on", async () => {
  const run = journey();
  await connectionsReady(run, false);
  const id = await startAsking(run);
  agentId = id;
  const pending = await until(() => pendingApprovals(run.stack, id), (rows) => rows.length > 0, "the tool call's approval", WAIT_MS[run.variant]);
  assert.equal(pending.length, 1, "one tool call waits");
  await open(run.window, "Inbox", "#/inbox");
  await run.window.getByRole("button", { name: "Approve", exact: true }).first().click();
  await confirmIfAsked(run.window, "Approve");
  await until(() => pendingApprovals(run.stack, id), (rows) => rows.length === 0, "the approval to be answered", WAIT_MS[run.variant]);
  const transcript = await until(() => transcriptText(run.stack, id), (text) => text.includes("approval-probe"), "the approved call's result", WAIT_MS[run.variant]);
  if (run.variant === "faux") await until(() => transcriptText(run.stack, id), (text) => text.includes(APPROVED_REPLY), "the agent to go on", WAIT_MS.faux);
  assert.ok(transcript.includes("approval-probe"), "the approved call ran");
});

test("approvals journey: a standing rule added from Controls", async () => {
  const run = journey();
  assert.ok(agentId !== undefined, "the first approvals test started the agent");
  try {
    const controls = await openControls(run.window, run.stack, agentId, "Approvals");
    await choose(run.window, await textField(await need(controls, "rule-tool")), "bash");
    const submit = await need(controls, "rule-submit");
    await expect(submit, "the rule is ready to add").toBeEnabled();
    await submit.click();
    await confirmIfAsked(run.window, "Add rule");
    const tools = await until(() => approvalRuleTools(run.stack), (now) => now.includes("bash"), "the rule for bash", 30_000);
    assert.equal(tools.filter((tool) => tool === "bash").length, 1, "one rule for bash was added");
  } finally {
    await cancelLive(run.stack, [agentId]);
  }
});
